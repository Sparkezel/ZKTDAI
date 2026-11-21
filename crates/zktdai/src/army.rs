//! The offense: factory production mix and the combat roles — raider parties
//! (box-in and harassment; the first 2 raiders scout), riot-skirmishers and
//! assault armies (front line, then pushes), artillery groups (back line), air,
//! and skittish scout units. The endgame trigger commits the whole army when the
//! enemy economy has not been seen for a long time.

use std::collections::{HashMap, HashSet};

use springai::{Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;
use crate::threat::Threat;
use crate::war_chest::{ProjectKind, ProjectState, WarChest};

/// Frames without a seen enemy mex/factory after which we go all-in (20 min).
const ENDGAME_QUIET_FRAMES: i32 = 36000;
/// Raiders that scout before joining raider parties.
const OPENING_SCOUTS: usize = 2;
/// Radius around our base checked for inbound incursions before pausing army
/// production for the first fusion (plan round 4 #4: gate on the threatmap).
const FUSION_PAUSE_SAFE_RADIUS: f32 = 1500.0;

/// Is this factory an air factory (builds flying units)?
fn is_air_factory(d: UnitDef) -> bool {
    d.name().contains("plane") || d.name().contains("gunship") || d.build_options().iter().any(|o| o.is_able_to_fly())
}

pub struct Army {
    cycle: usize,
    scout_ids: HashSet<i32>,
    scouts_assigned: usize,
    /// Last frame an enemy mex/factory was seen (0 = never).
    pub last_enemy_eco_seen: i32,
}

impl Army {
    pub fn new() -> Self {
        Army { cycle: 0, scout_ids: HashSet::new(), scouts_assigned: 0, last_enemy_eco_seen: 0 }
    }

    /// Track a newly created/finished unit of ours: assign opening scouts.
    pub fn on_our_unit(&mut self, ctx: &Ctx, unit: Unit, role: Role) {
        if role == Role::Raider && self.scouts_assigned < OPENING_SCOUTS && self.last_enemy_eco_seen == 0 {
            self.scout_ids.insert(unit.id());
            self.scouts_assigned += 1;
            ctx.cb.log(&format!("[army] {} is an opening scout", unit.id()));
        }
    }

    /// Enemy sightings: remember when we last saw enemy economy (mex/factory).
    pub fn note_enemy(&mut self, ctx: &Ctx, enemy: Unit) {
        let Some(d) = enemy.def() else { return };
        if d.is_able_to_move() {
            return;
        }
        let is_mex = ctx.is_mex(d);
        if is_mex || d.name().starts_with("factory") {
            self.last_enemy_eco_seen = ctx.frame;
        }
    }

    pub fn is_scout(&self, id: i32) -> bool {
        self.scout_ids.contains(&id)
    }

    /// True when the enemy economy has been unseen long enough to force the end
    /// (including never seen: 0 means "since game start").
    pub fn all_in(&self, ctx: &Ctx) -> bool {
        ctx.frame - self.last_enemy_eco_seen > ENDGAME_QUIET_FRAMES
    }

    pub fn push_ready(&self, ctx: &Ctx, roles: &HashMap<i32, Role>) -> bool {
        roles.values().filter(|r| r.is_combat()).count() as i32 >= ctx.opts.push_size
    }

    /// A factory that just went idle builds the next unit of the mix — unless
    /// a big energy project (first fusion) is active and production is paused.
    pub fn factory_idle(
        &mut self,
        ctx: &Ctx,
        factory: Unit,
        roles: &HashMap<i32, Role>,
        threat: &Threat,
        war: &mut WarChest,
        want_cons: usize,
    ) {
        // pause production only for the big energy push (first fusion, bump >= 0.5),
        // and only while no incursion is inbound near our base (threatmap gate,
        // plan round 4 #4 — pausing into a raid opens an army gap)
        if war
            .projects
            .iter()
            .any(|p| p.state == ProjectState::Active && p.kind == ProjectKind::Energy && p.budget_bump >= 0.5)
            && !threat.incursion_inbound(ctx.our_start, FUSION_PAUSE_SAFE_RADIUS)
        {
            return;
        }
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        // constructors: the planner decides how many we want (energy / base /
        // expansion / reclaim jobs). Only factories/plates/athena/engineer-com
        // can build cons, and only after a factory is placed (first is free).
        // But don't flood cons before the army exists: hold at 2 cons until we
        // have a few combat units, then let the planner's number through.
        let combat = roles.values().filter(|r| r.is_combat()).count();
        let con_cap = if combat < 4 { 2 } else { want_cons };
        let workers =
            cb.team_units().iter().filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && !u.is_being_built()).count();
        if workers < con_cap
            && let Some(con) = ctx.defs.con
            && war.can_spend(ctx, ProjectKind::Expansion, con.cost(metal))
            && cb.economy().current(metal) >= con.cost(metal)
        {
            let pos = factory.pos();
            factory.build(con, pos, 0, opt::NONE);
            return;
        }
        // air factories (factoryplane/factorygunship) build the air mix:
        // mostly fighters (Swift), every 4th a flying scout (Sparrow)
        let air = factory.def().is_some_and(is_air_factory);
        let Some(def) = self.next_unit(ctx, air, factory.def()) else { return };
        let cost = def.cost(metal);
        if cb.economy().current(metal) < cost {
            return;
        }
        // normal bucket: army production is capped at `army_share` of income per
        // window (greedy below 10 m/s); the rest floats as reserve or feeds porc
        if !war.can_spend(ctx, ProjectKind::Army, cost) {
            return;
        }
        let pos = factory.pos();
        if factory.build(def, pos, 0, opt::NONE) {
            war.note_spend(ProjectKind::Army, cost);
        }
    }

    /// Production weights per 100 builds: raiders dominate the opening
    /// (box-in; the first 2 scout), then the army shifts to skirm/assault/arty.
    /// Air factories instead cycle fighters with a scout share.
    fn next_unit(&mut self, ctx: &Ctx, air: bool, fac: Option<UnitDef>) -> Option<UnitDef> {
        if air {
            return self.next_air_unit(ctx, fac);
        }
        let d = &ctx.defs;
        let (wr, ws, wa, wt): (i32, i32, i32, i32) = if ctx.frame < 6000 { (70, 25, 0, 5) } else { (25, 30, 25, 20) };
        let total = wr + ws + wa + wt;
        let pick = (self.cycle as i32) % total;
        self.cycle += 1;
        let def = if pick < wr {
            d.raider
        } else if pick < wr + ws {
            d.skirm
        } else if pick < wr + ws + wa {
            d.assault
        } else {
            d.arty
        };
        def.or(d.raider).or(d.skirm).or(d.assault).or(d.arty)
    }

    /// Air mix for an air factory: prefer the named fighter/scout when the
    /// factory can build them, else any flying option; every 4th build is a
    /// scout. Falls back to the ground mix when nothing flies.
    fn next_air_unit(&mut self, ctx: &Ctx, fac: Option<UnitDef>) -> Option<UnitDef> {
        let opts: Vec<UnitDef> = fac.map(|d| d.build_options()).unwrap_or_default();
        let flying: Vec<UnitDef> = opts.iter().copied().filter(|o| o.is_able_to_fly()).collect();
        let in_opts = |d: Option<UnitDef>| d.filter(|d| opts.is_empty() || opts.contains(d));
        let scout =
            in_opts(ctx.defs.air_scout).or_else(|| flying.iter().copied().find(|o| ctx.defs.role(*o).is_scout()));
        let fighter = in_opts(ctx.defs.air)
            .or_else(|| flying.iter().copied().find(|o| o.is_able_to_attack() && !ctx.defs.role(*o).is_scout()));
        let fighter = fighter.or(scout);
        self.cycle += 1;
        let def = if self.cycle.is_multiple_of(4) { scout.or(fighter) } else { fighter.or(scout) };
        def.or_else(|| self.next_unit(ctx, false, fac))
    }

    pub fn combat_idle(&mut self, ctx: &Ctx, unit: Unit, role: Role, terr: &Territory, push: bool, fob: Option<Vec3>) {
        let cb = ctx.cb;
        let me = unit.pos();
        // hurt units and skittish scouts fall back to the forward base
        let home = fob.unwrap_or(ctx.our_start);
        let by_dist = |a: &Unit, b: &Unit| {
            a.pos().distance_2d(me).partial_cmp(&b.pos().distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
        };
        // scouts are skittish: retreat early, avoid armed enemies
        if self.is_scout(unit.id()) || role == Role::Scout {
            if unit.health() < unit.max_health() * 0.5 {
                unit.move_to(home, opt::NONE);
                return;
            }
            let armed = cb
                .enemy_units()
                .into_iter()
                .filter(|e| e.def().is_some_and(|d| d.is_able_to_attack()) && e.pos().distance_2d(me) < 600.0);
            if armed.clone().next().is_some() {
                unit.move_to(home, opt::NONE);
                return;
            }
            // engage only unprotected economy
            if let Some(t) = cb
                .enemy_units()
                .into_iter()
                .filter(|e| !e.def().is_some_and(|d| d.is_able_to_move()) && e.pos().distance_2d(me) < 500.0)
                .min_by(by_dist)
            {
                unit.attack(t, opt::NONE);
                return;
            }
            // otherwise scout deep toward the enemy base
            unit.fight(ctx.enemy_target, opt::NONE);
            return;
        }
        // engage anything close
        if let Some(t) = cb.enemy_units().into_iter().filter(|e| e.pos().distance_2d(me) < 800.0).min_by(by_dist) {
            unit.attack(t, opt::NONE);
            return;
        }
        match role {
            Role::Raider => {
                if ctx.opts.raiding {
                    let target = terr
                        .enemy_spots
                        .iter()
                        .min_by(|a, b| {
                            a.distance_2d(me).partial_cmp(&b.distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .copied()
                        .unwrap_or(ctx.enemy_target);
                    unit.fight(target, opt::NONE);
                } else {
                    unit.fight(terr.front, opt::NONE);
                }
            }
            Role::Arty => {
                let range = unit.max_range().max(1.0);
                match cb.enemy_units().into_iter().filter(|e| e.pos().distance_2d(me) < range * 0.9).min_by(by_dist) {
                    Some(t) => {
                        unit.attack(t, opt::NONE);
                    }
                    None => {
                        // hold a staging point behind the front
                        let dir = terr.enemy_centroid - terr.our_centroid;
                        let len = dir.length_2d().max(1.0);
                        let staging = terr.front + dir * (-300.0 / len);
                        unit.fight(staging, opt::NONE);
                    }
                }
            }
            _ => {
                if push {
                    // stage at the forward base, then go for the enemy base
                    let stage = fob.unwrap_or(terr.front);
                    if me.distance_2d(stage) > 800.0 {
                        unit.fight(stage, opt::NONE);
                    } else {
                        unit.fight(ctx.enemy_target, opt::NONE);
                    }
                } else {
                    unit.fight(terr.front, opt::NONE);
                }
            }
        }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory) {
        let combat = roles.values().filter(|r| r.is_combat()).count();
        let scouts = self.scout_ids.len();
        ctx.cb.log(&format!(
            "[army] frame={} combat={} scouts={} territory={:.0}% push_ready={} all_in={}",
            ctx.frame,
            combat,
            scouts,
            terr.share * 100.0,
            self.push_ready(ctx, roles),
            self.all_in(ctx)
        ));
    }
}
