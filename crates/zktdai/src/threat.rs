//! Threatmap v0 (issue #7): radar/LOS enemy density per region of the map and
//! incursion timers — "when could a raid reach each of our spots".
//!
//! v0 keeps one sighting per enemy *mobile combat* unit (radar+LOS), refreshed
//! from `enemy_units_in_radar_and_los` and from enemy events, and pruned once a
//! unit has not been re-seen for [`STALE_FRAMES`]. On top of that raw table it
//! answers the questions the other modules actually ask:
//!
//! - `enemy_near(pos, r)` / `incursion_inbound(pos, r)` — is there a tracked
//!   threat within `r` of a friendly position (porc decon must not reclaim into
//!   an inbound incursion; the fusion production pause is gated on this).
//! - `raid_eta(pos)` — frames until the fastest tracked enemy could reach `pos`
//!   (per-spot incursion timer).
//! - `army_near(pos, r)` — is one of our combat units within `r` (the "QRF
//!   close" half of local superiority).
//! - `local_superiority(pos, army_r, enemy_r)` — army within `army_r` OR no
//!   enemy on radar+LOS within `enemy_r`; gates naked caretaker placement and
//!   expansion constructors (#11 escort rule).
//!
//! The whole map is covered; "regions" are the 1024-elmo grid cells used for
//! the `hottest()` density report. Fail-open: with the `threatmap` option off
//! (or no sightings) every query says "no threat", which reproduces the
//! pre-threatmap behaviour of the callers.

use std::collections::HashMap;

use springai::{Unit, Vec3};

use crate::ai::Ctx;
use crate::defs::Role;

/// A sighting has not been refreshed for this many frames -> forget it (60 s).
const STALE_FRAMES: i32 = 1800;
/// Grid cell size (elmos) for the density report.
const CELL: f32 = 1024.0;
/// How often the periodic full scan runs (2 s at 30 fps).
pub const UPDATE_FRAMES: i32 = 60;
/// How often the `[threat]` status line is logged (30 s).
const LOG_FRAMES: i32 = 900;
/// An engagement stays "fresh" for this many frames (30 s) after the last hit.
const FIGHT_STALE_FRAMES: i32 = 900;

/// A fight we overheard: attacker -> target, at the positions where it happened.
/// Used for the operator board's 0.3-alpha directionality lines between engaged
/// units we are not currently tracking.
#[derive(Clone, Copy, Debug)]
struct Engagement {
    a: Vec3,
    b: Vec3,
    frame: i32,
}

/// Whether an engaged pair involves a unit we are not currently tracking.
/// Pure helper, unit-tested below.
fn involves_untracked(seen: &HashMap<i32, Sighting>, a: i32, b: i32) -> bool {
    !(seen.contains_key(&a) && seen.contains_key(&b))
}

/// One tracked enemy: last seen position, def speed (elmos/s, see sabotage),
/// and the frame it was last seen.
#[derive(Clone, Copy, Debug)]
struct Sighting {
    pos: Vec3,
    speed: f32,
    frame: i32,
}

/// Frames until an enemy moving at `speed` (elmos/s) covers `dist` elmos.
/// Pure helper, unit-tested below.
fn eta_frames(dist: f32, speed: f32) -> i32 {
    let s = speed.max(1.0);
    (dist / s * 30.0).ceil() as i32
}

/// Grid cell of a position (for the density report). Pure helper.
fn cell_of(pos: Vec3) -> (i32, i32) {
    ((pos.x / CELL) as i32, (pos.z / CELL) as i32)
}

pub struct Threat {
    /// Enemy unit id -> last sighting.
    seen: HashMap<i32, Sighting>,
    /// (attacker id, target id) -> last engagement (for the operator board).
    engaged: HashMap<(i32, i32), Engagement>,
    next_log: i32,
}

impl Threat {
    pub fn new() -> Self {
        Threat { seen: HashMap::new(), engaged: HashMap::new(), next_log: LOG_FRAMES }
    }

    /// Record a hit: `attacker` damaged `target` (from `UnitDamaged`).
    pub fn note_engaged(&mut self, ctx: &Ctx, attacker: Option<Unit>, target: Unit) {
        let Some(a) = attacker else { return };
        if a.id() == target.id() {
            return;
        }
        self.engaged.insert((a.id(), target.id()), Engagement { a: a.pos(), b: target.pos(), frame: ctx.frame });
    }

    /// Fresh engagement lines (attacker pos -> target pos) between units we are
    /// not currently tracking — for the operator board's faint directionality
    /// lines. Pairs where both endpoints are live-tracked are skipped: those
    /// fights are already visible as red dots on the board.
    pub fn fight_lines(&self, frame: i32) -> Vec<(Vec3, Vec3)> {
        self.engaged
            .iter()
            .filter(|(_, e)| frame - e.frame <= FIGHT_STALE_FRAMES)
            .filter(|((a, b), _)| involves_untracked(&self.seen, *a, *b))
            .map(|(_, e)| (e.a, e.b))
            .collect()
    }

    /// Immediate sighting from an enemy event (created / entered LOS / finished).
    pub fn note_enemy(&mut self, ctx: &Ctx, enemy: Unit) {
        if !ctx.opts.threatmap || !is_threat(ctx, &enemy) {
            return;
        }
        let speed = enemy.def().map(|d| d.speed()).unwrap_or(0.0);
        self.seen.insert(enemy.id(), Sighting { pos: enemy.pos(), speed, frame: ctx.frame });
    }

    /// Periodic full scan of radar+LOS plus pruning of stale sightings and
    /// stale engagements.
    pub fn update(&mut self, ctx: &Ctx) {
        self.engaged.retain(|_, e| ctx.frame - e.frame <= FIGHT_STALE_FRAMES);
        if !ctx.opts.threatmap {
            self.seen.clear();
            return;
        }
        for e in ctx.cb.enemy_units_in_radar_and_los() {
            if is_threat(ctx, &e) {
                let speed = e.def().map(|d| d.speed()).unwrap_or(0.0);
                self.seen.insert(e.id(), Sighting { pos: e.pos(), speed, frame: ctx.frame });
            }
        }
        self.seen.retain(|_, s| ctx.frame - s.frame <= STALE_FRAMES);
        if ctx.frame >= self.next_log {
            self.next_log = ctx.frame + LOG_FRAMES;
            let hot = self.hottest();
            ctx.cb.log(&format!(
                "[threat] frame={} tracked={} hottest={} eta_to_start={}",
                ctx.frame,
                self.seen.len(),
                match hot {
                    Some((pos, n)) => format!("({:.0},{:.0}):{n}", pos.x, pos.z),
                    None => "none".to_string(),
                },
                self.raid_eta(ctx.our_start),
            ));
        }
    }

    /// Any tracked threat within `radius` of `pos`?
    pub fn enemy_near(&self, pos: Vec3, radius: f32) -> bool {
        self.seen.values().any(|s| s.pos.distance_2d(pos) <= radius)
    }

    /// Is an enemy incursion inbound on the friendly position `pos`?
    pub fn incursion_inbound(&self, pos: Vec3, radius: f32) -> bool {
        self.enemy_near(pos, radius)
    }

    /// Frames until the fastest tracked enemy could reach `pos`
    /// (`i32::MAX` when nothing is tracked).
    pub fn raid_eta(&self, pos: Vec3) -> i32 {
        self.seen.values().map(|s| eta_frames(s.pos.distance_2d(pos), s.speed)).min().unwrap_or(i32::MAX)
    }

    /// One of our combat units within `army_r` of `pos` (the QRF half of
    /// local superiority).
    pub fn army_near(&self, ctx: &Ctx, roles: &HashMap<i32, Role>, pos: Vec3, army_r: f32) -> bool {
        ctx.cb
            .team_units()
            .iter()
            .any(|u| roles.get(&u.id()).is_some_and(|r| r.is_combat()) && u.pos().distance_2d(pos) <= army_r)
    }

    /// Local superiority at `pos`: our army is close (within `army_r`) OR no
    /// enemy is on radar+LOS within `enemy_r`. Gates naked caretakers and
    /// expansion constructors (plan round 2 #4, issue #11).
    pub fn local_superiority(
        &self,
        ctx: &Ctx,
        roles: &HashMap<i32, Role>,
        pos: Vec3,
        army_r: f32,
        enemy_r: f32,
    ) -> bool {
        self.army_near(ctx, roles, pos, army_r) || !self.enemy_near(pos, enemy_r)
    }

    /// Densiest grid cell: (centroid, count) — for the status line.
    fn hottest(&self) -> Option<(Vec3, usize)> {
        let mut cells: HashMap<(i32, i32), (f32, f32, usize)> = HashMap::new();
        for s in self.seen.values() {
            let c = cells.entry(cell_of(s.pos)).or_insert((0.0, 0.0, 0));
            c.0 += s.pos.x;
            c.1 += s.pos.z;
            c.2 += 1;
        }
        cells
            .into_iter()
            .max_by_key(|(_, (_, _, n))| *n)
            .filter(|(_, (_, _, n))| *n > 0)
            .map(|(_, (sx, sz, n))| (Vec3::xz(sx / n as f32, sz / n as f32), n))
    }

    /// How many enemies are currently tracked (for status_json).
    pub fn tracked(&self) -> usize {
        self.seen.len()
    }
}

/// A unit counts as a threat when it can move and shoot (raiders, skirm,
/// assault, arty, air, scouts). Statics are handled by sabotage's own
/// last-seen logic; here we care about incursions.
fn is_threat(ctx: &Ctx, u: &Unit) -> bool {
    let Some(d) = u.def() else { return false };
    if !d.is_able_to_move() {
        return false;
    }
    let role = ctx.defs.role(d);
    role.is_combat() || role.is_scout() || (d.is_able_to_attack() && d.speed() > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_scales_with_distance_and_speed() {
        // 60 elmos/s -> 2 s (60 frames) per 120 elmos
        assert_eq!(eta_frames(120.0, 60.0), 60);
        assert_eq!(eta_frames(120.0, 30.0), 120);
        // zero speed clamps to the 1 elmo/s floor, never divides by zero
        assert_eq!(eta_frames(30.0, 0.0), 900);
        // ceiling on partial frames
        assert_eq!(eta_frames(1.0, 60.0), 1);
    }

    #[test]
    fn untracked_pair_filter() {
        let mut seen = HashMap::new();
        seen.insert(7, Sighting { pos: Vec3::xz(0.0, 0.0), speed: 60.0, frame: 0 });
        // both tracked -> no line; one or both untracked -> line
        assert!(!involves_untracked(&seen, 7, 7));
        assert!(involves_untracked(&seen, 7, 8));
        assert!(involves_untracked(&seen, 8, 9));
        assert!(involves_untracked(&HashMap::new(), 7, 8));
    }

    #[test]
    fn cell_of_buckets_positions() {
        assert_eq!(cell_of(Vec3::xz(0.0, 0.0)), (0, 0));
        assert_eq!(cell_of(Vec3::xz(1023.9, 2047.9)), (0, 1));
        assert_eq!(cell_of(Vec3::xz(1024.0, 2048.0)), (1, 2));
        // the cast truncates toward zero, so small negative coords stay in cell 0
        assert_eq!(cell_of(Vec3::xz(-1.0, -1023.9)), (0, 0));
        assert_eq!(cell_of(Vec3::xz(-1024.0, -2048.0)), (-1, -2));
    }
}
