//! The AI root: owns all state and dispatches engine events to the modules.

use std::collections::{HashMap, HashSet};

use springai::{Callback, Event, NO_TIMEOUT, Resource, SkirmishAi, Unit, Vec3, opt};

use crate::army::Army;
use crate::defs::{Defs, Role, cmd};
use crate::economy::Economy;
use crate::fob::Fob;
use crate::fortify::Fortify;
use crate::porc::Porc;
use crate::sabotage::Sabotage;
use crate::territory::Territory;
use crate::war_chest::{WarChest, WarChestOpts};

/// Start-script options (declared in `data/AIOptions.lua`).
pub struct Opts {
    /// Percent of the map's metal spots to hold before turning offensive.
    pub territory_share: f32,
    /// Combat units gathered before an all-in push.
    pub push_size: i32,
    pub raiding: bool,
    pub porc: bool,
    pub sabotage: bool,
    /// Porc may spend at most this fraction of metal income per window.
    pub porc_budget_pct: f32,
    /// Metal reserve floor at game start (grows with storage afterwards).
    pub reserve_metal: f32,
    /// Desired metal stockpile (1500 + 300 headroom by default).
    pub stockpile_metal: f32,
    /// Strategy profile (option `strategy`; empty = picked by AI slot).
    pub strategy: String,
    pub walled_mexes: bool,
    pub caretakers: bool,
}

/// Shared per-frame game view the modules read.
pub struct Ctx {
    pub cb: Callback,
    pub defs: Defs,
    pub metal: Option<Resource>,
    pub energy: Option<Resource>,
    pub frame: i32,
    /// Metal spot positions, in elmos.
    pub spots: Vec<Vec3>,
    pub our_start: Vec3,
    /// Estimated enemy base (our start mirrored across the map centre).
    pub enemy_target: Vec3,
    /// Map wind bounds (elmos of wind; Zero-K caps max at 2.5).
    pub wind_min: f32,
    pub wind_max: f32,
    /// Geothermal feature positions.
    pub geo_spots: Vec<Vec3>,
    pub opts: Opts,
}

pub struct Zktdai {
    ctx: Ctx,
    /// Our live units by engine id -> role.
    roles: HashMap<i32, Role>,
    /// Metal spots a mex order was already issued for.
    ordered_spots: HashSet<usize>,
    start_sent: bool,
    terr: Territory,
    eco: Economy,
    fob: Fob,
    fort: Fortify,
    porc: Porc,
    sab: Sabotage,
    army: Army,
    war: WarChest,
}

fn bool_opt(cb: Callback, key: &str, default: bool) -> bool {
    cb.option(key).map(|v| matches!(v.as_str(), "true" | "1" | "yes" | "on")).unwrap_or(default)
}

impl SkirmishAi for Zktdai {
    fn new(cb: Callback) -> Self {
        let defs = Defs::new(cb);
        let metal = cb.resource_by_name("Metal");
        let energy = cb.resource_by_name("Energy");
        let opts = Opts {
            territory_share: cb.option("territory_share").and_then(|v| v.parse().ok()).unwrap_or(50.0),
            push_size: cb.option("push_size").and_then(|v| v.parse().ok()).unwrap_or(12),
            raiding: bool_opt(cb, "raiding", true),
            porc: bool_opt(cb, "porc", true),
            sabotage: bool_opt(cb, "sabotage", true),
            porc_budget_pct: cb.option("porc_budget_pct").and_then(|v| v.parse().ok()).unwrap_or(20.0) / 100.0,
            reserve_metal: cb.option("reserve_metal").and_then(|v| v.parse().ok()).unwrap_or(0.0),
            stockpile_metal: cb.option("stockpile_metal").and_then(|v| v.parse().ok()).unwrap_or(1800.0),
            strategy: cb.option("strategy").filter(|s| !s.is_empty()).unwrap_or_else(|| {
                const PROFILES: &[&str] = &["balanced", "econ_greedy", "porc_heavy"];
                PROFILES[(cb.ai_id().unsigned_abs() as usize) % PROFILES.len()].to_string()
            }),
            walled_mexes: bool_opt(cb, "walled_mexes", true),
            caretakers: bool_opt(cb, "caretakers", true),
        };
        let war = WarChest::new(
            cb,
            WarChestOpts {
                porc_cap_pct: opts.porc_budget_pct,
                stockpile_target: opts.stockpile_metal,
                reserve_metal_start: opts.reserve_metal,
                strategy: opts.strategy.clone(),
            },
        );
        let map = cb.map();
        let centre = Vec3::xz(map.width() as f32 * 4.0, map.height() as f32 * 4.0);
        Zktdai {
            ctx: Ctx {
                cb,
                defs,
                metal,
                energy,
                frame: 0,
                spots: Vec::new(),
                our_start: centre,
                enemy_target: centre,
                wind_min: 0.0,
                wind_max: 0.0,
                geo_spots: Vec::new(),
                opts,
            },
            roles: HashMap::new(),
            ordered_spots: HashSet::new(),
            start_sent: false,
            terr: Territory::new(0, centre, centre),
            eco: Economy::new(),
            fob: Fob::new(),
            fort: Fortify::new(),
            porc: Porc::new(),
            sab: Sabotage::new(),
            army: Army::new(),
            war,
        }
    }

    fn handle_event(&mut self, event: Event) -> i32 {
        match event {
            Event::Init { saved_game } => {
                let cb = self.ctx.cb;
                let map = cb.map();
                if let Some(metal) = self.ctx.metal {
                    self.ctx.spots = map.resource_map_spots(metal);
                }
                self.ctx.wind_min = map.wind_min();
                self.ctx.wind_max = map.wind_max();
                self.ctx.geo_spots = cb
                    .features()
                    .iter()
                    .filter(|f| f.def().is_some_and(|d| d.name().to_lowercase().contains("geo")))
                    .map(|f| f.pos())
                    .collect();
                self.terr = Territory::new(self.ctx.spots.len(), self.ctx.our_start, self.ctx.enemy_target);
                cb.log(&format!(
                    "[zktdai] init saved={saved_game} build={} engine={} game={} map='{}' {}x{} spots={} defs={}",
                    env!("ZKTDAI_BUILD"),
                    cb.engine_version(),
                    cb.mod_info().human_name(),
                    map.name(),
                    map.width(),
                    map.height(),
                    self.ctx.spots.len(),
                    cb.unit_defs().len(),
                ));
                cb.log(&format!(
                    "[zktdai] key defs by human name: Lotus={} Stinger={} Lucifer={} Aspis={}",
                    self.ctx.defs.by_human("Lotus").is_some(),
                    self.ctx.defs.by_human("Stinger").is_some(),
                    self.ctx.defs.by_human("Lucifer").is_some(),
                    self.ctx.defs.by_human("Aspis").is_some(),
                ));
            }
            Event::Update { frame } => {
                self.ctx.frame = frame;
                if !self.start_sent {
                    self.send_start();
                }
                if frame % 30 == 0 {
                    self.war.update(&self.ctx);
                }
                if frame % 60 == 0 {
                    self.terr.update(&self.ctx, &self.roles);
                    let owned = self.terr.owned.clone();
                    self.ordered_spots.retain(|i| !owned[*i]);
                }
                if frame % 150 == 0 {
                    self.porc.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                }
                if frame % 120 == 0 {
                    self.sab.update(&self.ctx, &self.roles, &self.terr);
                }
                if frame % 900 == 0 {
                    self.fort.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                }
                if frame % 300 == 0 {
                    self.fob.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                    self.eco.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                    self.army.update(&self.ctx, &self.roles, &self.terr);
                }
            }
            Event::UnitCreated { unit, builder } => {
                self.war.on_unit_created(&self.ctx, unit, builder);
                self.porc.note_created(unit, self.ctx.frame);
                self.track(unit);
            }
            Event::UnitFinished { unit } => {
                self.war.on_unit_finished(&self.ctx, unit);
                // all metal extractors run High priority (cmd 34220 state 2):
                // they get their metal/overdrive share first
                if unit.team() == self.ctx.cb.game().my_team()
                    && unit.def().is_some_and(|d| self.ctx.metal.is_some_and(|m| d.extracts_resource(m) > 0.0))
                {
                    unit.execute_custom_command(cmd::PRIORITY, &[2.0], opt::NONE, NO_TIMEOUT);
                }
                self.track(unit);
            }
            Event::UnitIdle { unit } => self.unit_idle(unit),
            Event::UnitDestroyed { unit, .. } => {
                self.war.on_unit_destroyed(&self.ctx, unit);
                self.roles.remove(&unit.id());
            }
            Event::UnitGiven { unit, new_team, .. } | Event::UnitCaptured { unit, new_team, .. } => {
                if new_team == self.ctx.cb.game().my_team() {
                    self.track(unit); // captured/given to us: it is ours now
                } else {
                    self.roles.remove(&unit.id());
                }
            }
            Event::EnemyDestroyed { enemy, .. } => self.sab.forget(enemy.id()),
            Event::EnemyEnterLos { enemy } | Event::EnemyCreated { enemy } | Event::EnemyFinished { enemy } => {
                self.army.note_enemy(&self.ctx, enemy);
            }
            Event::WeaponFired { unit, .. } => self.porc.note_combat(unit, self.ctx.frame),
            Event::UnitDamaged { unit, .. } => {
                self.porc.note_damage(&self.ctx, unit.pos(), self.ctx.frame);
                self.maybe_retreat(unit);
            }
            Event::LuaMessage { data } => {
                // operator widget poll (issue #14): answer with a JSON status
                if data.trim() == "ZKTDAI_STATUS" {
                    let json = self.status_json();
                    self.ctx.cb.log(&format!("[zktdai] status {}", json));
                    let _ = self.ctx.cb.call_lua_rules(&format!("ZKTDAI_STATUS_REPLY {}", json));
                }
            }
            Event::Release { reason } => self.ctx.cb.log(&format!("[zktdai] release reason={reason}")),
            _ => {}
        }
        0
    }
}

impl Zktdai {
    /// Combined status JSON for the operator widget: war chest + army + territory.
    fn status_json(&self) -> String {
        let war = self.war.status_json(&self.ctx);
        format!(
            "{{\"army\":{{\"units\":{},\"scouts\":{}}},\"territory\":{{\"share\":{:.3},\"front\":[{:.0},{:.0}]}},\"war\":{}}}",
            self.roles.values().filter(|r| r.is_combat()).count(),
            self.roles.values().filter(|r| **r == Role::Scout).count(),
            self.terr.share,
            self.terr.front.x,
            self.terr.front.z,
            &war[1..] // strip outer '{' to nest
        )
    }

    fn track(&mut self, unit: Unit) {
        if unit.team() != self.ctx.cb.game().my_team() {
            return;
        }
        if let Some(def) = unit.def() {
            let role = self.ctx.defs.role(def);
            self.roles.insert(unit.id(), role);
            self.army.on_our_unit(&self.ctx, unit, role);
        }
    }

    fn unit_idle(&mut self, unit: Unit) {
        let Some(role) = self.roles.get(&unit.id()).copied() else { return };
        match role {
            Role::Commander => self.eco.commander_idle(
                &self.ctx,
                unit,
                &self.roles,
                &mut self.ordered_spots,
                &mut self.terr,
                &self.war,
            ),
            Role::Worker => self.eco.worker_idle(
                &self.ctx,
                unit,
                &mut self.ordered_spots,
                &mut self.terr,
                &self.war,
                &self.fob.mobile,
            ),
            Role::Factory => self.army.factory_idle(&self.ctx, unit, &self.roles, &self.war),
            r if r.is_combat() || r.is_scout() => {
                let push = self.army.push_ready(&self.ctx, &self.roles) || self.army.all_in(&self.ctx);
                self.army.combat_idle(&self.ctx, unit, r, &self.terr, push, self.fob.pos);
            }
            _ => {}
        }
    }

    /// Hurt units retreat to the forward base once it exists, else to start.
    fn retreat_point(&self) -> Vec3 {
        self.fob.pos.unwrap_or(self.ctx.our_start)
    }

    fn maybe_retreat(&mut self, unit: Unit) {
        if self.roles.get(&unit.id()).is_none_or(|r| !r.is_combat()) {
            return;
        }
        if unit.health() < unit.max_health() * 0.35 {
            unit.move_to(self.retreat_point(), opt::NONE);
        }
    }

    /// Pick the start position: the metal spot nearest the map centre.
    fn send_start(&mut self) {
        let cb = self.ctx.cb;
        let map = cb.map();
        let centre = Vec3::xz(map.width() as f32 * 4.0, map.height() as f32 * 4.0);
        let pos = self
            .ctx
            .spots
            .iter()
            .min_by(|a, b| {
                a.distance_2d(centre).partial_cmp(&b.distance_2d(centre)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .unwrap_or(centre);
        let pos = Vec3::new(pos.x, map.elevation_at(pos.x, pos.z), pos.z);
        if cb.game().send_start_position(true, pos) {
            self.start_sent = true;
            self.ctx.our_start = pos;
            self.ctx.enemy_target = Vec3::xz(map.width() as f32 * 8.0 - pos.x, map.height() as f32 * 8.0 - pos.z);
            self.terr = Territory::new(self.ctx.spots.len(), pos, self.ctx.enemy_target);
            cb.log(&format!(
                "[zktdai] start at ({:.0},{:.0}), enemy estimate ({:.0},{:.0})",
                pos.x, pos.z, self.ctx.enemy_target.x, self.ctx.enemy_target.z
            ));
        }
    }
}
