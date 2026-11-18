//! Sabotage: when an enemy critical target is spotted without defenses, send
//! a surgical strike at it — raiders/skirmishers for near targets, more of
//! them (or artillery) for far ones.

use std::collections::HashMap;

use springai::{Unit, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;

/// Substrings of internal unit names that mark a static as critical, with a
/// value score. First match wins.
const VALUE_HINTS: &[(&str, f32)] = &[
    ("antinuke", 100.0),
    ("nuke", 100.0),
    ("factory", 80.0),
    ("fusion", 70.0),
    ("singu", 60.0),
    ("geo", 50.0),
    ("mex", 30.0),
    ("storage", 20.0),
    ("solar", 15.0),
    ("wind", 15.0),
    ("pylon", 15.0),
    ("radar", 10.0),
];

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
            let Some(&(_, value)) = VALUE_HINTS.iter().find(|(hint, _)| name.contains(*hint)) else { continue };
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
