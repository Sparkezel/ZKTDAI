//! Sabotage: when an enemy critical target is spotted without defenses, send
//! a surgical strike at it — raiders/skirmishers for near targets, more of
//! them (or artillery) for far ones.

use std::collections::HashMap;

use springai::{Unit, opt};

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

pub struct Sabotage {
    /// Enemy unit id -> frame of the last strike dispatched at it.
    dispatched: HashMap<i32, i32>,
}

impl Sabotage {
    pub fn new() -> Self {
        Sabotage { dispatched: HashMap::new() }
    }

    pub fn forget(&mut self, id: i32) {
        self.dispatched.remove(&id);
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, _terr: &Territory) {
        if !ctx.opts.sabotage {
            return;
        }
        let cb = ctx.cb;
        let enemies = cb.enemy_units_in_radar_and_los();
        let mut best: Option<(Unit, f32)> = None;
        for e in &enemies {
            let Some(d) = e.def() else { continue };
            if d.is_able_to_move() {
                continue; // statics only
            }
            let name = d.name();
            let Some(&(hint, hint_value)) = VALUE_HINTS.iter().find(|(hint, _)| name.contains(*hint)) else { continue };
            let hint_value = if hint == "factory" { factory_value(ctx.frame) } else { hint_value };
            // heavies (non-flying, cost > 1000; commanders < 4000 exempt) are worth a strike too
            let value = hint_value.max(if ctx.defs.is_heavy(d) { 90.0 } else { 0.0 });
            let defended = enemies.iter().any(|o| {
                o.id() != e.id()
                    && o.def().is_some_and(|od| od.is_able_to_move() && od.is_able_to_attack())
                    && o.pos().distance_2d(e.pos()) < DEFENSE_RADIUS
            });
            if defended {
                continue;
            }
            if self.dispatched.get(&e.id()).is_some_and(|&f| ctx.frame - f < REDISPATCH_FRAMES) {
                continue;
            }
            // prefer deeper targets (into enemy territory) — antinukes, heavies
            let depth_bonus = e.pos().distance_2d(ctx.our_start) * 0.02;
            if best.as_ref().is_none_or(|(_, v)| value + depth_bonus > *v) {
                best = Some((*e, value + depth_bonus));
            }
        }
        let Some((target, value)) = best else { return };
        let pos = target.pos();
        let mut strike: Vec<Unit> = cb
            .team_units()
            .into_iter()
            .filter(|u| roles.get(&u.id()).is_some_and(|r| r.is_combat()) && u.num_current_commands() == 0)
            .collect();
        strike.sort_by(|a, b| {
            a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
        });
        // far targets get a bigger strike
        let n = strike.len().min(if pos.distance_2d(ctx.our_start) > 2000.0 { 6 } else { 4 });
        for u in strike.iter().take(n) {
            u.attack(target, opt::NONE);
        }
        self.dispatched.insert(target.id(), ctx.frame);
        cb.log(&format!(
            "[sabotage] frame={} strike of {n} at {} ({:.0},{:.0}) value={value:.0}",
            ctx.frame,
            target.def().map(|d| d.name()).unwrap_or_default(),
            pos.x,
            pos.z
        ));
    }
}
