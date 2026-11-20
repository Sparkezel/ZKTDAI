//! Forward base (FOB): a point near the front that is the retreat point for
//! hurt units, the staging ground for pushes, and the anchor for heavy porc.
//!
//! The base must hold at least [`MIN_BP`] buildpower. Mobile constructors are
//! assigned first (they stay reassignable for quick reclaim/build jobs); static
//! caretakers expand the base's static buildpower on demand, and once the base
//! holds [`MIN_BP`] static buildpower all but one mobile constructor is freed.
//! The base is supplied with a buried Hacksaw and a pit Stinger (pit dug first;
//! the nanoframe is placed only once the dig is verified complete), and can
//! escalate into a heavy turret (Desolator/Lucifer) — an existing heavy turret
//! near the front becomes the beachhead the base is sited around.

use std::collections::{HashMap, HashSet};

use springai::{Callback, Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::economy::build_at;
use crate::terraform;
use crate::territory::Territory;
use crate::war_chest::{ProjectKind, ProjectState, WarChest};

/// Minimum buildpower the base holds.
const MIN_BP: f32 = 30.0;
/// Depth of the pits the base's Hacksaw and Stinger sit in.
const PIT_DEPTH: f32 = 40.0;
/// Radius around the base position that counts as "at the base".
const BASE_RADIUS: f32 = 500.0;
/// How far behind the front the base is sited.
const SITE_BEHIND_FRONT: f32 = 350.0;
/// A pit being dug; the nanoframe goes down only once it is complete.
struct PendingPit {
    pos: Vec3,
    /// Heightmap value at the site when the dig started.
    start_elev: f32,
    /// Depth asked for (positive).
    depth: f32,
    /// Def to build once the pit is done.
    def: UnitDef,
    kind: &'static str,
    dug_frame: i32,
}

pub struct Fob {
    /// Base position, once sited.
    pub pos: Option<Vec3>,
    /// Mobile constructors currently assigned to hold the base.
    pub mobile: HashSet<i32>,
    /// Pit currently being dug (one at a time), if any.
    pending: Option<PendingPit>,
    pit_cooldown: i32,
    next: i32,
}

fn build_power(d: UnitDef) -> f32 {
    d.build_speed().max(1.0)
}

fn count_def_near(cb: Callback, def: Option<UnitDef>, pos: Vec3, r: f32) -> usize {
    cb.team_units().iter().filter(|u| u.def() == def && u.pos().distance_2d(pos) <= r).count()
}

fn idle_worker_near(cb: Callback, roles: &HashMap<i32, Role>, pos: Vec3, max_dist: f32) -> Option<Unit> {
    cb.team_units()
        .into_iter()
        .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
        .filter(|u| u.pos().distance_2d(pos) <= max_dist)
        .min_by(|a, b| {
            a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn is_heavy_turret(d: &UnitDef) -> bool {
    let n = d.name();
    !d.is_able_to_move() && (n == "turretantiheavy" || n == "turretheavy")
}

impl Fob {
    pub fn new() -> Self {
        Fob { pos: None, mobile: HashSet::new(), pending: None, pit_cooldown: 0, next: 1200 }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        if ctx.frame < self.next {
            return;
        }
        self.next = ctx.frame + 300;
        let income = cb.economy().income(metal);

        // Site the base: an existing heavy turret near the front is the beachhead.
        if self.pos.is_none() {
            let dir = terr.enemy_centroid - terr.our_centroid;
            let len = dir.length_2d().max(1.0);
            let want = terr.front + dir * (-SITE_BEHIND_FRONT / len);
            let beachhead = cb
                .team_units()
                .into_iter()
                .find(|u| u.def().is_some_and(|d| is_heavy_turret(&d)) && u.pos().distance_2d(terr.front) <= 800.0)
                .map(|u| u.pos());
            let pos = beachhead.unwrap_or(want);
            self.pos = Some(pos);
            cb.log(&format!("[fob] frame={} base at ({:.0},{:.0})", ctx.frame, pos.x, pos.z));
        }
        let pos = self.pos.unwrap();

        // Prune dead assignments.
        let alive: HashSet<i32> = cb.team_units().iter().map(|u| u.id()).collect();
        self.mobile.retain(|id| alive.contains(id));

        // Buildpower accounting.
        let caretaker_bp = ctx.defs.caretaker.map(build_power).unwrap_or(16.0);
        let static_bp = count_def_near(cb, ctx.defs.caretaker, pos, BASE_RADIUS) as f32 * caretaker_bp;
        let mobile_bp: f32 = cb
            .team_units()
            .iter()
            .filter(|u| self.mobile.contains(&u.id()))
            .filter_map(|u| u.def())
            .map(build_power)
            .sum();

        // Mobile constructors first: reach MIN_BP by assigning idle workers.
        let mut need = MIN_BP - (static_bp + mobile_bp);
        while need > 0.0 {
            let w = cb
                .team_units()
                .into_iter()
                .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
                .filter(|u| !self.mobile.contains(&u.id()))
                .min_by(|a, b| {
                    a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
                });
            let Some(w) = w else { break };
            need -= w.def().map(build_power).unwrap_or(10.0);
            self.mobile.insert(w.id());
            w.move_to(pos, opt::NONE);
        }

        // Static buildpower expands on demand while below MIN_BP.
        if ctx.opts.caretakers
            && static_bp < MIN_BP
            && let Some(ca) = ctx.defs.caretaker
        {
            let cost = ca.cost(metal);
            if war.can_spend(ctx, ProjectKind::Porc, cost)
                && let Some(w) = idle_worker_near(cb, roles, pos, 1200.0)
                && build_at(cb, w, ca, pos, 250.0)
            {
                war.note_spend(ProjectKind::Porc, cost);
                cb.log(&format!("[fob] frame={} caretaker at base ({:.0},{:.0})", ctx.frame, pos.x, pos.z));
            }
        }

        // Once the base has MIN_BP static buildpower, free all but one mobile constructor.
        if static_bp >= MIN_BP && self.mobile.len() > 1 {
            let keep = cb
                .team_units()
                .into_iter()
                .filter(|u| self.mobile.contains(&u.id()))
                .min_by(|a, b| {
                    a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|u| u.id());
            if let Some(k) = keep {
                self.mobile.retain(|id| *id == k);
                cb.log(&format!(
                    "[fob] frame={} static bp {:.0} >= {MIN_BP}: freed mobile constructors",
                    ctx.frame, static_bp
                ));
            }
        }

        // Buried Hacksaw (frontline AA; base AA stays flat), then pit Stinger.
        // The pit is dug FIRST; the nanoframe is placed only once the dig is
        // complete, verified against the live heightmap (the gadget writes it
        // incrementally as metal is spent).
        let next_pit: Option<(UnitDef, &'static str)> = match ctx.defs.get("turretaaclose") {
            Some(h) if count_def_near(cb, Some(h), pos, BASE_RADIUS) == 0 && income >= 15.0 => Some((h, "hacksaw")),
            _ => ctx
                .defs
                .get("turretheavylaser")
                .filter(|s| count_def_near(cb, Some(*s), pos, BASE_RADIUS) == 0 && income >= 40.0 && terr.share >= 0.5)
                .map(|s| (s, "stinger")),
        };
        let mut clear_pending = false;
        if let Some(p) = &self.pending {
            let dug = p.start_elev - cb.map().elevation_at(p.pos.x, p.pos.z);
            if dug >= p.depth * 0.8 {
                let cost = p.def.cost(metal);
                if war.can_spend(ctx, ProjectKind::Porc, cost)
                    && let Some(w) = idle_worker_near(cb, roles, p.pos, 1200.0)
                    && build_at(cb, w, p.def, p.pos, 120.0)
                {
                    war.note_spend(ProjectKind::Porc, cost);
                    cb.log(&format!(
                        "[fob] frame={} {} placed in pit at ({:.0},{:.0}) after {:.0}/{:.0} dug",
                        ctx.frame, p.kind, p.pos.x, p.pos.z, dug, p.depth
                    ));
                }
                clear_pending = true;
            } else if ctx.frame - p.dug_frame > 9000 {
                cb.log(&format!(
                    "[fob] frame={} pit for {} timed out (dug {:.0}/{:.0})",
                    ctx.frame, p.kind, dug, p.depth
                ));
                clear_pending = true;
                self.pit_cooldown = ctx.frame + 3000;
            }
        }
        if clear_pending {
            self.pending = None;
        }
        if self.pending.is_none()
            && ctx.frame >= self.pit_cooldown
            && let Some((def, kind)) = next_pit
        {
            let cost = def.cost(metal);
            if war.can_spend(ctx, ProjectKind::Porc, cost) && self.dig_pit(cb, roles, pos) {
                self.pending = Some(PendingPit {
                    pos,
                    start_elev: cb.map().elevation_at(pos.x, pos.z),
                    depth: PIT_DEPTH,
                    def,
                    kind,
                    dug_frame: ctx.frame,
                });
            }
        }

        // Escalate into a heavy turret: Lucifer at high income, else Desolator.
        let heavy = if income >= 60.0 { ctx.defs.get("turretantiheavy") } else { ctx.defs.get("turretheavy") };
        if let Some(h) = heavy {
            let have_heavy = cb
                .team_units()
                .iter()
                .any(|u| u.def().is_some_and(|d| is_heavy_turret(&d)) && u.pos().distance_2d(pos) <= 700.0);
            let project_active = war.projects.iter().any(|p| {
                p.kind == ProjectKind::HeavyPorc && matches!(p.state, ProjectState::Queued | ProjectState::Active)
            });
            let cost = h.cost(metal);
            if !have_heavy
                && !project_active
                && income >= 40.0
                && war.request_priority(ctx, "fob-heavy", ProjectKind::HeavyPorc, Some(h), cost, 0.3)
                && let Some(w) = idle_worker_near(cb, roles, pos, 1600.0)
            {
                build_at(cb, w, h, pos, 300.0);
            }
        }
    }

    /// Dig a `PIT_DEPTH` pit across the site so a turret built there is buried.
    /// Returns true if the dig job was issued. The gadget skips structure
    /// footprints, so the turret keeps its height while the ground around it
    /// drops.
    fn dig_pit(&self, cb: Callback, roles: &HashMap<i32, Role>, pos: Vec3) -> bool {
        let builders: Vec<Unit> = cb
            .team_units()
            .into_iter()
            .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
            .filter(|u| u.pos().distance_2d(pos) <= 1200.0)
            .take(2)
            .collect();
        if builders.is_empty() {
            return false;
        }
        let pts = terraform::line_points(Vec3::xz(pos.x - 60.0, pos.z), Vec3::xz(pos.x + 60.0, pos.z), 30.0);
        let issued = terraform::terraform(cb, &builders, &pts, -PIT_DEPTH, terraform::next_tag());
        if issued {
            cb.log(&format!("[fob] digging pit -{PIT_DEPTH:.0} at ({:.0},{:.0})", pos.x, pos.z));
        }
        issued
    }
}
