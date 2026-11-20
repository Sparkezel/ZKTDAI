//! Sabotage: when an enemy critical target is spotted without defenses, send
//! a surgical strike at it — raiders/skirmishers for near targets, more of
//! them (or artillery) for far ones.
//!
//! Targets are remembered: a radar jammer (or any critical static) that leaves
//! our vision stays in `seen` at its last-seen position (statics never move), so
//! we keep attacking where it stands even while it is hidden.

use std::collections::{HashMap, HashSet};

use springai::{Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;

/// Substrings of internal unit names that mark a static as critical, with a
/// value score. First match wins (`antinuke` before `nuke`, `heavygeo` before
/// `geo`, `heavyradar` before `radar`). The nuke silo is the first target; the
/// radars sit below the singularity — blinding the enemy beats most targets, but
/// not superweapons. The `factory` entry decays with game time, see
/// `factory_value`.
const VALUE_HINTS: &[(&str, f32)] = &[
    ("antinuke", 100.0),
    ("nuke", 110.0),      // strategic nuke silo: first building to be attacked
    ("heavygeo", 90.0),   // advanced geothermal (energyheavygeo)
    ("singu", 90.0),      // singularity
    ("heavyradar", 85.0), // Advanced Radar (staticheavyradar)
    ("radar", 80.0),      // Radar Tower (staticradar)
    ("jammer", 75.0),     // Radar Jammer (Cornea, staticjammer)
    ("fusion", 70.0),
    ("geo", 60.0), // geothermal
    ("factory", 90.0),
    ("mex", 30.0),
    ("storage", 20.0),
    ("solar", 15.0),
    ("wind", 15.0),
    ("pylon", 15.0),
];

/// Factory value over time: 90 at game start, −10 every 2 minutes, floor 40
/// from 10 minutes on (a factory still matters late, just less than a superweapon).
fn factory_value(frame: i32) -> f32 {
    let steps = (frame / 3600).min(5);
    90.0 - 10.0 * steps as f32
}

/// How close an armed enemy mobile must be to count as defending a target.
const DEFENSE_RADIUS: f32 = 700.0;
/// Minimum frames before the same target is struck again.
const REDISPATCH_FRAMES: i32 = 600;

/// Strike value of a static def, or None if it is not a target. The `factory`
/// hint decays with game time; heavies (non-flying, cost > 1000; commanders
/// < 4000 exempt) are worth a strike even without a hint.
fn target_value(ctx: &Ctx, d: UnitDef) -> Option<f32> {
    let name = d.name();
    let base = match VALUE_HINTS.iter().find(|(hint, _)| name.contains(*hint)) {
        Some(&(hint, v)) => {
            if hint == "factory" {
                factory_value(ctx.frame)
            } else {
                v
            }
        }
        None => 0.0,
    };
    let heavy = if ctx.defs.is_heavy(d) { 90.0 } else { 0.0 };
    let v = base.max(heavy);
    (v > 0.0).then_some(v)
}

pub struct Sabotage {
    /// Enemy unit id -> frame of the last strike dispatched at it.
    dispatched: HashMap<i32, i32>,
    /// Remembered critical statics: id -> (def, last-seen pos, frame). Statics
    /// never move, so the last-seen position stays exact; entries die with
    /// `EnemyDestroyed` (`forget`).
    seen: HashMap<i32, (UnitDef, Vec3, i32)>,
}

impl Sabotage {
    pub fn new() -> Self {
        Sabotage { dispatched: HashMap::new(), seen: HashMap::new() }
    }

    pub fn forget(&mut self, id: i32) {
        self.dispatched.remove(&id);
        self.seen.remove(&id);
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, _terr: &Territory) {
        if !ctx.opts.sabotage {
            return;
        }
        let cb = ctx.cb;
        let enemies = cb.enemy_units_in_radar_and_los();
        // Refresh memory with everything visible now. A jammer that leaves our
        // vision stays remembered at its last-seen position.
        for e in &enemies {
            let Some(d) = e.def() else { continue };
            if d.is_able_to_move() || target_value(ctx, d).is_none() {
                continue; // statics worth striking only
            }
            self.seen.insert(e.id(), (d, e.pos(), ctx.frame));
        }
        // All targets are static and never move: if the engine no longer knows the
        // unit, or we have vision at the remembered spot and no building is there,
        // assume it was destroyed and drop it from memory.
        let visible: HashSet<i32> = enemies.iter().map(|e| e.id()).collect();
        let mut gone: Vec<(i32, Vec3)> = Vec::new();
        for (&id, &(_, pos, _)) in &self.seen {
            let unknown = Unit::from_id(cb, id).is_none();
            let unseen_with_vision = !visible.contains(&id) && !cb.friendly_units_in(pos, 700.0).is_empty();
            if unknown || unseen_with_vision {
                gone.push((id, pos));
            }
        }
        for (id, pos) in gone {
            cb.log(&format!(
                "[sabotage] frame={} target {} at ({:.0},{:.0}) assumed destroyed",
                ctx.frame, id, pos.x, pos.z
            ));
            self.forget(id);
        }
        // Pick the best remembered target; its defenders are checked against what
        // is visible now near the remembered position.
        let mut best: Option<(i32, UnitDef, Vec3, f32)> = None;
        for (&id, &(d, pos, _seen)) in &self.seen {
            let Some(value) = target_value(ctx, d) else { continue };
            let defended = enemies.iter().any(|o| {
                o.id() != id
                    && o.def().is_some_and(|od| od.is_able_to_move() && od.is_able_to_attack())
                    && o.pos().distance_2d(pos) < DEFENSE_RADIUS
            });
            if defended {
                continue;
            }
            if self.dispatched.get(&id).is_some_and(|&f| ctx.frame - f < REDISPATCH_FRAMES) {
                continue;
            }
            // prefer deeper targets (into enemy territory) — antinukes, heavies, jammers
            let score = value + pos.distance_2d(ctx.our_start) * 0.02;
            if best.as_ref().is_none_or(|&(_, _, _, s)| score > s) {
                best = Some((id, d, pos, score));
            }
        }
        let Some((id, def, pos, value)) = best else { return };
        let mut strike: Vec<Unit> = cb
            .team_units()
            .into_iter()
            // sabotage strikes are fast mobile ground only: raiders, skirmishers and
            // assaults faster than 60 elmo/s. No artillery (it shells such positions
            // from afar) and no air for now.
            .filter(|u| {
                roles.get(&u.id()).is_some_and(|r| matches!(r, Role::Raider | Role::Skirm | Role::Assault))
                    && u.num_current_commands() == 0
                    && u.def().is_some_and(|d| d.speed() > 60.0)
            })
            .collect();
        strike.sort_by(|a, b| {
            a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
        });
        // far targets get a bigger strike
        let n = strike.len().min(if pos.distance_2d(ctx.our_start) > 2000.0 { 6 } else { 4 });
        // Resolve the unit if the engine still tracks it; otherwise attack the
        // remembered position (hidden/jammed target).
        let target = Unit::from_id(cb, id);
        // A hidden cloaked target (the radar jammer, Cornea: initCloaked with
        // minCloakDistance/decloak_distance = 100 in staticjammer.lua) must be
        // decloaked before it can be seen: the nearest strike unit stands within
        // half the decloak radius (and within 40 elmos) of the remembered spot.
        let decloak_stand = if target.is_none() && def.is_start_cloaked() {
            let decloak = def.decloak_distance();
            Some(if decloak > 0.0 { (0.5 * decloak).min(40.0) } else { 40.0 })
        } else {
            None
        };
        for (k, u) in strike.iter().take(n).enumerate() {
            if k == 0
                && let Some(stand) = decloak_stand
            {
                let dir = ctx.our_start - pos;
                let len = dir.length_2d().max(1.0);
                u.move_to(pos + dir * (stand / len), opt::NONE);
            } else {
                match target {
                    Some(t) => {
                        u.attack(t, opt::NONE);
                    }
                    None => {
                        u.attack_area(pos, 120.0, opt::NONE);
                    }
                }
            }
        }
        self.dispatched.insert(id, ctx.frame);
        cb.log(&format!(
            "[sabotage] frame={} strike of {n} at {} ({:.0},{:.0}) value={value:.0}{}",
            ctx.frame,
            def.name(),
            pos.x,
            pos.z,
            if target.is_some() { "" } else { " (remembered)" }
        ));
    }
}
