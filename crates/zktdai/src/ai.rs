//! The AI root: owns all state and dispatches engine events to the modules.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::json;
use springai::{Callback, Event, NO_TIMEOUT, Resource, SkirmishAi, Unit, Vec3, opt};

/// Event-trace logging (set `ZKTDAI_TRACE_EVENTS=1` in the game process env;
/// used by tools/headless to diagnose event delivery, e.g. the v0.1 idle bug):
/// log every non-update event plus every 300th update for the first minute.
fn trace_events() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::var("ZKTDAI_TRACE_EVENTS").is_ok_and(|v| v != "0"))
}

use crate::army::Army;
use crate::defs::{Defs, Role, cmd};
use crate::economy::{Economy, has_queued_mex_build};
use crate::fob::Fob;
use crate::fortify::Fortify;
use crate::operator::{self, BoardEnemy, BoardSpot, BoardState, BoardUnit, Operator, UnitMark};
use crate::planner::Planner;
use crate::porc::Porc;
use crate::sabotage::Sabotage;
use crate::territory::Territory;
use crate::threat::{Threat, UPDATE_FRAMES};
use crate::war_chest::{ProjectState, WarChest, WarChestOpts};

/// Start-script options (declared in `data/AIOptions.lua`).
pub struct Opts {
    /// Percent of the map's metal spots to hold before turning offensive.
    pub territory_share: f32,
    /// Combat units gathered before an all-in push.
    pub push_size: i32,
    pub raiding: bool,
    pub porc: bool,
    /// Front-line terraforming ditch (porc). Off by default for now: early
    /// game the front is just the map-centre estimate, so workers ditch the
    /// midmap instead of expanding.
    pub ditch: bool,
    pub sabotage: bool,
    /// Porc may spend at most this fraction of metal income per window.
    pub porc_budget_pct: f32,
    /// Army share of normal-priority income per window (normal bucket; greedy
    /// below 10 m/s).
    pub army_share: f32,
    /// Metal reserve floor at game start (grows with storage afterwards).
    pub reserve_metal: f32,
    /// Desired metal stockpile (1500 + 300 headroom by default).
    pub stockpile_metal: f32,
    /// Strategy profile (option `strategy`; empty = picked by AI slot).
    pub strategy: String,
    pub walled_mexes: bool,
    pub caretakers: bool,
    /// Threatmap v0 on/off (issue #7); off = fail-open, queries report no threat.
    pub threatmap: bool,
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
    /// Metal yield per spot (`mex_metal<i>` rules param, default 2.0).
    pub spot_metal: Vec<f32>,
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

impl Ctx {
    /// True for metal extractors. ZK mexes don't set the engine's
    /// `extractsMetal` tag (they use the `ismex` custom param and per-spot
    /// rules params), so `extracts_resource` always returns 0 for them —
    /// detect mexes by the configured mex def or the `ismex` param instead.
    pub fn is_mex(&self, d: springai::UnitDef) -> bool {
        self.defs.mex.is_some_and(|m| d.id() == m.id()) || d.custom_param("ismex").is_some()
    }
}

pub struct Zktdai {
    ctx: Ctx,
    /// Our live units by engine id -> role.
    roles: HashMap<i32, Role>,
    /// Metal spots a mex order was already issued for.
    ordered_spots: HashSet<usize>,
    start_sent: bool,
    /// Set once our_start was anchored to the commander's real spawn pos.
    anchored: bool,
    terr: Territory,
    eco: Economy,
    fob: Fob,
    fort: Fortify,
    porc: Porc,
    sab: Sabotage,
    army: Army,
    planner: Planner,
    threat: Threat,
    war: WarChest,
    /// Live dashboard server (option `operator` / env `ZKTDAI_OPERATOR`);
    /// disabled = no server, no files.
    op: Operator,
    /// Cached terrain layer for the board PNG.
    op_terrain: Option<operator::Terrain>,
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
            ditch: bool_opt(cb, "ditch", false),
            porc: bool_opt(cb, "porc", true),
            sabotage: bool_opt(cb, "sabotage", true),
            porc_budget_pct: cb.option("porc_budget_pct").and_then(|v| v.parse().ok()).unwrap_or(20.0) / 100.0,
            army_share: cb.option("army_share").and_then(|v| v.parse().ok()).unwrap_or(0.6),
            reserve_metal: cb.option("reserve_metal").and_then(|v| v.parse().ok()).unwrap_or(0.0),
            stockpile_metal: cb.option("stockpile_metal").and_then(|v| v.parse().ok()).unwrap_or(1800.0),
            strategy: cb.option("strategy").filter(|s| !s.is_empty()).unwrap_or_else(|| {
                const PROFILES: &[&str] = &["balanced", "econ_greedy", "porc_heavy"];
                PROFILES[(cb.ai_id().unsigned_abs() as usize) % PROFILES.len()].to_string()
            }),
            walled_mexes: bool_opt(cb, "walled_mexes", true),
            caretakers: bool_opt(cb, "caretakers", true),
            threatmap: bool_opt(cb, "threatmap", true),
        };
        let war = WarChest::new(
            cb,
            WarChestOpts {
                porc_cap_pct: opts.porc_budget_pct,
                army_share: opts.army_share,
                stockpile_target: opts.stockpile_metal,
                reserve_metal_start: opts.reserve_metal,
                strategy: opts.strategy.clone(),
            },
        );
        let map = cb.map();
        let centre = Vec3::xz(map.width() as f32 * 4.0, map.height() as f32 * 4.0);
        let op = Operator::start(cb);
        Zktdai {
            ctx: Ctx {
                cb,
                defs,
                metal,
                energy,
                frame: 0,
                spots: Vec::new(),
                spot_metal: Vec::new(),
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
            anchored: false,
            terr: Territory::new(0, centre, centre),
            eco: Economy::new(),
            fob: Fob::new(),
            fort: Fortify::new(),
            porc: Porc::new(),
            sab: Sabotage::new(),
            army: Army::new(),
            planner: Planner::new(),
            threat: Threat::new(),
            war,
            op,
            op_terrain: None,
        }
    }

    fn handle_event(&mut self, event: Event) -> i32 {
        if trace_events() && self.ctx.frame < 1800 {
            let name = event.name();
            match &event {
                Event::CommandFinished { unit, command_id, command_topic } => self.ctx.cb.log(&format!(
                    "[zktdai] event commandFinished f={} unit={} cmd={command_id} topic={command_topic}",
                    self.ctx.frame,
                    unit.id()
                )),
                Event::UnitCreated { unit, .. } | Event::UnitFinished { unit } => {
                    if self.ctx.frame < 20000 {
                        let d = unit.def().map(|d| d.name().to_string()).unwrap_or_default();
                        self.ctx.cb.log(&format!(
                            "[zktdai] event {name} f={} unit={} def={} built={}",
                            self.ctx.frame,
                            unit.id(),
                            d,
                            !unit.is_being_built()
                        ));
                    }
                }
                _ if name != "update" || self.ctx.frame % 300 == 0 => {
                    self.ctx.cb.log(&format!("[zktdai] event {name} f={}", self.ctx.frame));
                }
                _ => {}
            }
        }
        match event {
            Event::Init { saved_game } => {
                let cb = self.ctx.cb;
                let map = cb.map();
                // Metal spots: prefer the game's published rules params
                // (mex_count / mex_x<i> / mex_y<i> / mex_z<i>, set by ZK's
                // mex_spot_finder gadget). Those are the coordinates the
                // mex-placement gadget allows mex builds on — the engine's
                // own metalmap spots can differ by enough to get orders
                // denied. Fall back to the engine metalmap when absent.
                let g = cb.game();
                let n = g.rules_param_float("mex_count", 0.0) as i32;
                if n > 0 {
                    self.ctx.spots = (1..=n)
                        .map(|i| {
                            Vec3::new(
                                g.rules_param_float(&format!("mex_x{i}"), 0.0),
                                g.rules_param_float(&format!("mex_y{i}"), 0.0),
                                g.rules_param_float(&format!("mex_z{i}"), 0.0),
                            )
                        })
                        .collect();
                    self.ctx.spot_metal = (1..=n).map(|i| g.rules_param_float(&format!("mex_metal{i}"), 2.0)).collect();
                    cb.log(&format!("[zktdai] {n} metal spots from rules params"));
                }
                if self.ctx.spots.is_empty()
                    && let Some(metal) = self.ctx.metal
                {
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
                self.op.tick(frame);
                if self.op.enabled() {
                    if frame % 30 == 0 {
                        self.op.set_snapshot(self.snapshot_json());
                    }
                    if frame % 150 == 0
                        && let Some(png) = self.render_board_png()
                    {
                        self.op.set_board(png);
                    }
                    if frame % 300 == 0 {
                        self.op.write_files();
                    }
                }
                if !self.start_sent {
                    self.send_start();
                }
                if trace_events() && frame == 0 {
                    for u in self.ctx.cb.team_units() {
                        let opts: Vec<String> = u
                            .def()
                            .map(|d| d.build_options().iter().map(|o| o.name().to_string()).collect())
                            .unwrap_or_default();
                        self.ctx.cb.log(&format!(
                            "[zktdai] buildopts u={} def={} n={} {:?}",
                            u.id(),
                            u.def().map(|d| d.name().to_string()).unwrap_or_default(),
                            opts.len(),
                            opts
                        ));
                    }
                }
                if frame % 30 == 0 {
                    self.war.update(&self.ctx);
                }
                if frame % UPDATE_FRAMES == 0 {
                    self.terr.update(&self.ctx, &self.roles);
                    self.threat.update(&self.ctx);
                    let owned = self.terr.owned.clone();
                    self.ordered_spots.retain(|i| !owned[*i]);
                }
                // The engine only sends unitIdle when a command *finishes*, so a
                // unit that never got an order (the opening commander) would wait
                // forever. Sweep idle constructors/commander/factories periodically
                // and dispatch the same handler (idempotent: skips units with orders).
                // A mex order can be silently denied (mex-placement rules) and
                // never produce a nanoframe — don't let that block the spot
                // forever: forget orders whose spot still has no mex of ours.
                if frame % 1200 == 0
                    && let Some(mex) = self.ctx.defs.mex
                {
                    let cb = self.ctx.cb;
                    let spots = self.ctx.spots.clone();
                    let owned = self.terr.owned.clone();
                    let mut expired = Vec::new();
                    let roles = self.roles.clone();
                    self.ordered_spots.retain(|i| {
                        let spot = spots[*i];
                        // keep the order if the mex is up, or a builder is still
                        // en route / has the build queued — only a truly dead
                        // order (nobody near, nothing queued) is denied
                        let keep = owned[*i]
                            || cb.team_units().iter().any(|u| u.def() == Some(mex) && u.pos().distance_2d(spot) < 96.0)
                            || cb.team_units().iter().any(|u| {
                                matches!(roles.get(&u.id()), Some(Role::Commander) | Some(Role::Worker))
                                    && (has_queued_mex_build(u, mex, spot) || u.pos().distance_2d(spot) < 400.0)
                            });
                        if !keep {
                            expired.push(*i);
                        }
                        keep
                    });
                    // Orders that expired without a nanoframe were silently
                    // denied (claimed spot, dead builder): don't re-order them
                    // for a while, or one con burns the whole game on one spot.
                    for i in expired {
                        self.terr.blocked[i] = frame;
                        let spot = spots[i];
                        let enemy_mex = cb
                            .enemy_units_in_radar_and_los()
                            .iter()
                            .filter(|e| e.def().is_some_and(|d| self.ctx.is_mex(d)))
                            .map(|e| e.pos().distance_2d(spot))
                            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        cb.log(&format!(
                            "[zktdai] mex order on spot {i} ({:.0},{:.0}) expired unbuilt (nearest enemy mex {:.0}), blocking 3600f",
                            spot.x,
                            spot.z,
                            enemy_mex.unwrap_or(-1.0)
                        ));
                    }
                }
                if frame % 120 == 0 {
                    let idle: Vec<i32> = self
                        .ctx
                        .cb
                        .team_units()
                        .into_iter()
                        .filter(|u| {
                            !u.is_being_built()
                                && u.num_current_commands() == 0
                                && matches!(
                                    self.roles.get(&u.id()),
                                    Some(Role::Commander) | Some(Role::Worker) | Some(Role::Factory)
                                )
                        })
                        .map(|u| u.id())
                        .collect();
                    if frame % 600 == 0 && !idle.is_empty() {
                        self.ctx.cb.log(&format!(
                            "[zktdai] idle sweep f={frame} units={idle:?} roles={:?}",
                            idle.iter().map(|id| self.roles.get(id).copied()).collect::<Vec<_>>()
                        ));
                    }
                    for id in idle {
                        if let Some(u) = Unit::from_id(self.ctx.cb, id) {
                            self.unit_idle(u);
                        }
                    }
                }
                // probe: every builder's queue + metal while the opening is in flight
                if trace_events()
                    && frame < 20000
                    && frame % 30 == 0
                    && let Some(m) = self.ctx.metal
                {
                    let metal = self.ctx.cb.economy().current(m);
                    for u in self.ctx.cb.team_units() {
                        let Some(r) = self.roles.get(&u.id()) else { continue };
                        if !matches!(r, Role::Commander | Role::Worker | Role::Factory) || u.is_being_built() {
                            continue;
                        }
                        let cmds: Vec<String> =
                            u.current_commands().iter().map(|c| format!("{}|p={:?}", c.id(), c.params())).collect();
                        let p = u.pos();
                        self.ctx.cb.log(&format!(
                            "[probe] f={frame} {r:?} u={} pos=({:.0},{:.0}) metal={metal:.1} ncmd={} cmds={cmds:?}",
                            u.id(),
                            p.x,
                            p.z,
                            u.num_current_commands()
                        ));
                    }
                }
                if frame % 150 == 0 {
                    self.porc.update(&self.ctx, &self.roles, &self.terr, &self.threat, &mut self.war);
                }
                if frame % 120 == 0 {
                    self.sab.update(&self.ctx, &self.roles, &self.terr);
                }
                if frame % 900 == 0 {
                    self.fort.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                }
                if frame % 300 == 0 {
                    self.fob.update(&self.ctx, &self.roles, &self.terr, &self.threat, &mut self.war);
                    self.eco.update(&self.ctx, &self.roles, &self.terr, &mut self.war);
                    self.army.update(&self.ctx, &self.roles, &self.terr);
                    self.planner.update(
                        &self.ctx,
                        &self.roles,
                        &self.terr,
                        &mut self.ordered_spots,
                        &self.threat,
                        &mut self.war,
                        &self.fob.mobile,
                    );
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
                if unit.team() == self.ctx.cb.game().my_team() && unit.def().is_some_and(|d| self.ctx.is_mex(d)) {
                    unit.execute_custom_command(cmd::PRIORITY, &[2.0], opt::NONE, NO_TIMEOUT);
                }
                self.track(unit);
                // The engine only sends unitIdle when a *command finishes*
                // (CommandAI.cpp), never for a unit that had no orders — so a
                // freshly finished commander/worker/factory would wait forever.
                // A finished unit is idle: dispatch the same handler.
                self.unit_idle(unit);
            }
            Event::UnitIdle { unit } => self.unit_idle(unit),
            Event::UnitDestroyed { unit, .. } => {
                self.war.on_unit_destroyed(&self.ctx, unit);
                if self.ctx.metal.is_some_and(|m| unit.def().is_some_and(|d| d.cost(m) >= 50.0)) {
                    self.planner.note_wreck(unit.pos());
                }
                self.roles.remove(&unit.id());
            }
            Event::UnitGiven { unit, new_team, .. } | Event::UnitCaptured { unit, new_team, .. } => {
                if new_team == self.ctx.cb.game().my_team() {
                    self.track(unit); // captured/given to us: it is ours now
                } else {
                    self.roles.remove(&unit.id());
                }
            }
            Event::EnemyDestroyed { enemy, .. } => {
                self.sab.forget(enemy.id());
                if self.ctx.metal.is_some_and(|m| enemy.def().is_some_and(|d| d.cost(m) >= 50.0)) {
                    self.planner.note_wreck(enemy.pos());
                }
            }
            Event::EnemyEnterLos { enemy } | Event::EnemyCreated { enemy } | Event::EnemyFinished { enemy } => {
                self.army.note_enemy(&self.ctx, enemy);
                self.threat.note_enemy(&self.ctx, enemy);
            }
            Event::WeaponFired { unit, .. } => self.porc.note_combat(unit, self.ctx.frame),
            Event::UnitDamaged { unit, attacker, .. } => {
                self.porc.note_damage(&self.ctx, unit.pos(), self.ctx.frame);
                self.threat.note_engaged(&self.ctx, attacker, unit);
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
            Event::Release { reason } => {
                self.ctx.cb.log(&format!("[zktdai] release reason={reason}"));
                // Stop the operator server before the engine may unload the DLL.
                self.op.shutdown();
            }
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
            "{{\"army\":{{\"units\":{},\"scouts\":{}}},\"territory\":{{\"share\":{:.3},\"front\":[{:.0},{:.0}]}},\"threat\":{{\"tracked\":{}}},\"war\":{}}}",
            self.roles.values().filter(|r| r.is_combat()).count(),
            self.roles.values().filter(|r| **r == Role::Scout).count(),
            self.terr.share,
            self.terr.front.x,
            self.terr.front.z,
            self.threat.tracked(),
            &war[1..] // strip outer '{' to nest
        )
    }

    /// Full world snapshot for the operator dashboard (`GET /snapshot`).
    fn snapshot_json(&self) -> String {
        let cb = self.ctx.cb;
        let map = cb.map();
        let game = cb.game();
        let eco = cb.economy();
        let res = |m: Option<Resource>| -> serde_json::Value {
            m.map(|m| {
                json!({
                    "current": eco.current(m),
                    "income": eco.income(m),
                    "usage": eco.usage(m),
                    "storage": eco.storage(m),
                })
            })
            .unwrap_or(serde_json::Value::Null)
        };
        let war: serde_json::Value =
            serde_json::from_str(&self.war.status_json(&self.ctx)).unwrap_or(serde_json::Value::Null);
        let units: Vec<serde_json::Value> = cb
            .team_units()
            .iter()
            .map(|u| {
                let p = u.pos();
                let d = u.def();
                json!({
                    "id": u.id(),
                    "def": d.map(|d| d.name()).unwrap_or_default(),
                    "role": self.roles.get(&u.id()).map(|r| format!("{r:?}")).unwrap_or_default(),
                    "x": p.x,
                    "z": p.z,
                    "hp": u.health(),
                    "max_hp": u.max_health(),
                    "building": u.is_being_built(),
                    "static": d.is_some_and(|d| !d.is_able_to_move()),
                })
            })
            .collect();
        let enemies: Vec<serde_json::Value> = cb
            .enemy_units_in_radar_and_los()
            .iter()
            .map(|e| {
                let p = e.pos();
                json!({
                    "id": e.id(),
                    "def": e.def().map(|d| d.name()).unwrap_or_default(),
                    "x": p.x,
                    "z": p.z,
                    "static": e.def().is_some_and(|d| !d.is_able_to_move()),
                })
            })
            .collect();
        let spots: Vec<serde_json::Value> = self
            .ctx
            .spots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "i": i,
                    "x": s.x,
                    "z": s.z,
                    "metal": self.ctx.spot_metal.get(i).copied().unwrap_or(2.0),
                    "owned": self.terr.owned.get(i).copied().unwrap_or(false),
                    "enemy": self.terr.enemy.get(i).copied().unwrap_or(false),
                    "blocked": self.terr.blocked.get(i).copied().unwrap_or(-1),
                })
            })
            .collect();
        json!({
            "name": "ZKTDAI",
            "version": env!("CARGO_PKG_VERSION"),
            "frame": self.ctx.frame,
            "map": {"name": map.name(), "w": map.width(), "h": map.height()},
            "team": game.my_team(),
            "ally_team": game.my_ally_team(),
            "metal": res(self.ctx.metal),
            "energy": res(self.ctx.energy),
            "territory": {
                "share": self.terr.share,
                "owned": self.terr.owned.iter().filter(|o| **o).count(),
                "enemy": self.terr.enemy.iter().filter(|e| **e).count(),
                "front": [self.terr.front.x, self.terr.front.z],
                "our_start": [self.ctx.our_start.x, self.ctx.our_start.z],
                "enemy_target": [self.ctx.enemy_target.x, self.ctx.enemy_target.z],
            },
            "spots": spots,
            "units": units,
            "enemies": enemies,
            "army": {
                "units": self.roles.values().filter(|r| r.is_combat()).count(),
                "scouts": self.roles.values().filter(|r| **r == Role::Scout).count(),
            },
            "threat": {"tracked": self.threat.tracked()},
            "war": war,
            // Projects window: who is dedicated to what right now.
            "commander": {
                "opening": self.eco.opening_tag(),
                "mex_spot": self.eco.com_mex_spot(),
            },
            "jobs": self
                .planner
                .job_groups()
                .iter()
                .map(|(j, ids)| json!({"job": format!("{j:?}"), "units": ids}))
                .collect::<Vec<_>>(),
            "projects": self
                .war
                .projects
                .iter()
                .filter(|p| p.state != ProjectState::Done && p.state != ProjectState::Failed)
                .map(|p| {
                    json!({
                        "name": p.name,
                        "kind": format!("{:?}", p.kind),
                        "state": format!("{:?}", p.state),
                        "builder": p.builder,
                        "target": p.target.map(|u| u.id()),
                        "reserved": p.reserved,
                    })
                })
                .collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// Render the board PNG (map + spots + units + enemies + front). Called
    /// every ~5 s when the operator is on.
    fn render_board_png(&mut self) -> Option<Vec<u8>> {
        let cb = self.ctx.cb;
        let map = cb.map();
        if self.op_terrain.is_none() {
            self.op_terrain = Some(operator::sample_terrain(map));
        }
        let terrain = self.op_terrain.as_ref()?;
        let frame = self.ctx.frame;
        let state = BoardState {
            map_w: map.width() as f32 * 8.0,
            frame,
            spots: self
                .ctx
                .spots
                .iter()
                .enumerate()
                .map(|(i, s)| BoardSpot {
                    x: s.x,
                    z: s.z,
                    owned: self.terr.owned.get(i).copied().unwrap_or(false),
                    enemy: self.terr.enemy.get(i).copied().unwrap_or(false),
                    blocked: self.terr.blocked.get(i).is_some_and(|b| *b >= 0 && frame - *b < 3600),
                })
                .collect(),
            geo: self.ctx.geo_spots.clone(),
            units: cb
                .team_units()
                .iter()
                .map(|u| {
                    let p = u.pos();
                    let mark = match self.roles.get(&u.id()).copied().unwrap_or(Role::Other) {
                        Role::Commander => UnitMark::Commander,
                        Role::Worker => UnitMark::Worker,
                        Role::Factory => UnitMark::Building,
                        _ if u.def().is_some_and(|d| !d.is_able_to_move()) => UnitMark::Building,
                        _ => UnitMark::Combat,
                    };
                    BoardUnit { x: p.x, z: p.z, mark }
                })
                .collect(),
            enemies: cb
                .enemy_units_in_radar_and_los()
                .iter()
                .map(|e| {
                    let p = e.pos();
                    BoardEnemy { x: p.x, z: p.z, is_static: e.def().is_some_and(|d| !d.is_able_to_move()) }
                })
                .collect(),
            our_start: self.ctx.our_start,
            front: self.terr.front,
            enemy_target: self.ctx.enemy_target,
            fight_lines: self.threat.fight_lines(frame),
        };
        Some(operator::render_board(&state, terrain))
    }

    fn track(&mut self, unit: Unit) {
        if unit.team() != self.ctx.cb.game().my_team() {
            return;
        }
        if let Some(def) = unit.def() {
            let role = self.ctx.defs.role(def);
            self.roles.insert(unit.id(), role);
            self.army.on_our_unit(&self.ctx, unit, role);
            // Anchor our base to where the commander actually spawned: the
            // start pos we send is only a hint, the engine may place us in a
            // startbox instead (observed in headless games).
            if role == Role::Commander && !self.anchored {
                self.anchored = true;
                let pos = unit.pos();
                self.ctx.our_start = pos;
                self.ctx.enemy_target = Vec3::xz(
                    self.ctx.cb.map().width() as f32 * 8.0 - pos.x,
                    self.ctx.cb.map().height() as f32 * 8.0 - pos.z,
                );
                self.terr = Territory::new(self.ctx.spots.len(), pos, self.ctx.enemy_target);
                self.ctx.cb.log(&format!(
                    "[zktdai] anchored base at commander pos ({:.0},{:.0}), enemy estimate ({:.0},{:.0})",
                    pos.x, pos.z, self.ctx.enemy_target.x, self.ctx.enemy_target.z
                ));
            }
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
                &self.roles,
                &mut self.ordered_spots,
                &mut self.terr,
                &self.war,
                &self.fob.mobile,
                &self.planner,
            ),
            Role::Factory => {
                let want = self.planner.wanted_cons(&self.ctx, &self.terr);
                self.army.factory_idle(&self.ctx, unit, &self.roles, &self.threat, &mut self.war, want);
            }
            r if r.is_combat() || r.is_scout() => {
                let push = self.army.push_ready(&self.ctx, &self.roles) || self.army.all_in(&self.ctx);
                self.army.combat_idle(&self.ctx, unit, r, &self.roles, &self.terr, &self.threat, push, self.fob.pos);
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
            // The commander's UnitCreated event can arrive before the first
            // update, anchoring our_start to the real spawn pos — don't
            // clobber that with the merely *requested* start pos.
            if !self.anchored {
                self.ctx.our_start = pos;
                self.ctx.enemy_target = Vec3::xz(map.width() as f32 * 8.0 - pos.x, map.height() as f32 * 8.0 - pos.z);
                self.terr = Territory::new(self.ctx.spots.len(), pos, self.ctx.enemy_target);
            }
            cb.log(&format!(
                "[zktdai] start at ({:.0},{:.0}), anchored ({:.0},{:.0}), enemy estimate ({:.0},{:.0})",
                pos.x,
                pos.z,
                self.ctx.our_start.x,
                self.ctx.our_start.z,
                self.ctx.enemy_target.x,
                self.ctx.enemy_target.z
            ));
        }
    }
}
