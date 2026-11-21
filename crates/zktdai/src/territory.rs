//! Territory: which metal spots we hold, where the front line is, and which
//! spot to expand to next. The doctrine target is ~50% of the map's spots
//! (`territory_share` option); past that, expansion stops and the army turns
//! fully offensive.

use std::collections::{HashMap, HashSet};

use springai::Vec3;

use crate::ai::Ctx;
use crate::defs::Role;

pub struct Territory {
    /// Per metal spot (index into `Ctx::spots`): a friendly mex works it.
    pub owned: Vec<bool>,
    /// Per metal spot: an enemy mex works it. Last-known: a spot whose enemy
    /// mex left LOS stays enemy until the spot is seen empty (same ownership
    /// model as ZK's gui_metal_territories widget).
    pub enemy: Vec<bool>,
    /// Frame the spot was last seen held by an enemy finished mex, -1 if the
    /// spot is known neutral.
    pub enemy_seen: Vec<i32>,
    /// Power-diagram weight per spot: `0.25 * min_pair_dist * clamp(metal/2,
    /// 0.5, 2.5)` — high-yield spots claim bigger cells, crowded spots smaller
    /// (same weighting as gui_metal_territories' BuildCells).
    pub weights: Vec<f32>,
    pub our_centroid: Vec3,
    pub enemy_centroid: Vec3,
    /// Midpoint between the two centroids — the front line.
    pub front: Vec3,
    /// Share of all spots we own, 0..1.
    pub share: f32,
    /// Enemy-held spots, for raider targets.
    pub enemy_spots: Vec<Vec3>,
    /// Frame of the last creep-claim above the territory target.
    pub last_creep: i32,
}

impl Territory {
    pub fn new(n_spots: usize, our_start: Vec3, enemy_target: Vec3) -> Self {
        Territory {
            owned: vec![false; n_spots],
            enemy: vec![false; n_spots],
            enemy_seen: vec![-1; n_spots],
            weights: vec![0.0; n_spots],
            our_centroid: our_start,
            enemy_centroid: enemy_target,
            front: Vec3::xz((our_start.x + enemy_target.x) / 2.0, (our_start.z + enemy_target.z) / 2.0),
            share: 0.0,
            enemy_spots: Vec::new(),
            last_creep: 0,
        }
    }

    /// Is the map position currently in our line of sight? Uses the LOS map at
    /// `los_mip_level` resolution.
    fn pos_visible(ctx: &Ctx, pos: Vec3) -> bool {
        let map = ctx.cb.map();
        let mip = ctx.cb.mod_info().los_mip_level().max(0);
        let w = map.width() >> mip;
        let h = map.height() >> mip;
        if w <= 0 || h <= 0 {
            return false;
        }
        let mut los = Vec::new();
        map.los_map_into(&mut los);
        if los.is_empty() {
            return false;
        }
        let x = ((pos.x as i32) >> mip).clamp(0, w - 1) as usize;
        let z = ((pos.z as i32) >> mip).clamp(0, h - 1) as usize;
        los.get(z * w as usize + x).copied().unwrap_or(0) != 0
    }

    fn nearest_spot(&self, ctx: &Ctx, pos: Vec3, max_dist: f32) -> Option<usize> {
        ctx.spots
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.distance_2d(pos)))
            .filter(|(_, d)| *d <= max_dist)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    }

    /// Recompute ownership and the front from live units. Call every ~2 s.
    pub fn update(&mut self, ctx: &Ctx, _roles: &HashMap<i32, Role>) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let radius = cb.map().extractor_radius(metal).max(1.0);
        self.owned.iter_mut().for_each(|o| *o = false);
        // Enemy ownership is last-known: only cleared once the spot is seen
        // empty (a finished mex owns a spot; a nanoframe in sight is neutral).
        let frame = ctx.frame;
        let (mut ox, mut oz, mut on) = (0.0, 0.0, 0);
        for u in cb.team_units() {
            let Some(d) = u.def() else { continue };
            if !d.is_able_to_move()
                && !u.is_being_built()
                && ctx.is_mex(d)
                && let Some(i) = self.nearest_spot(ctx, u.pos(), radius)
            {
                self.owned[i] = true;
                ox += ctx.spots[i].x;
                oz += ctx.spots[i].z;
                on += 1;
            }
        }
        let (mut ex, mut ez, mut en) = (0.0, 0.0, 0);
        let mut seen_now = vec![false; self.enemy.len()];
        for e in cb.enemy_units_in_radar_and_los() {
            let Some(d) = e.def() else { continue };
            if !d.is_able_to_move()
                && !e.is_being_built()
                && ctx.is_mex(d)
                && let Some(i) = self.nearest_spot(ctx, e.pos(), radius)
            {
                seen_now[i] = true;
                self.enemy_seen[i] = frame;
                ex += ctx.spots[i].x;
                ez += ctx.spots[i].z;
                en += 1;
            }
        }
        // Spots last seen enemy-held: keep them enemy unless we can currently
        // see the spot and no enemy mex is on it.
        for (i, seen) in seen_now.iter().enumerate() {
            if self.enemy_seen[i] >= 0 {
                if *seen {
                    self.enemy[i] = true;
                } else if Self::pos_visible(ctx, ctx.spots[i]) {
                    self.enemy[i] = false;
                    self.enemy_seen[i] = -1;
                } else {
                    self.enemy[i] = true; // last known owner, out of sight
                }
            } else {
                self.enemy[i] = false;
            }
        }
        // Power-diagram weights from the spots' metal yields.
        if self.weights.len() != ctx.spots.len() {
            self.weights = vec![0.0; ctx.spots.len()];
        }
        let mut nearest = f32::MAX;
        for a in &ctx.spots {
            for b in &ctx.spots {
                if !std::ptr::eq(a, b) {
                    nearest = nearest.min(a.distance_2d(*b));
                }
            }
        }
        if nearest == f32::MAX {
            nearest = 1.0;
        }
        for (i, _s) in ctx.spots.iter().enumerate() {
            let metal = ctx.spot_metal.get(i).copied().unwrap_or(2.0);
            self.weights[i] = 0.25 * nearest * (metal / 2.0).clamp(0.5, 2.5);
        }
        if on > 0 {
            self.our_centroid = Vec3::xz(ox / on as f32, oz / on as f32);
        }
        if en > 0 {
            self.enemy_centroid = Vec3::xz(ex / en as f32, ez / en as f32);
        }
        self.front = Vec3::xz(
            (self.our_centroid.x + self.enemy_centroid.x) / 2.0,
            (self.our_centroid.z + self.enemy_centroid.z) / 2.0,
        );
        let total = ctx.spots.len().max(1);
        self.share = self.owned.iter().filter(|o| **o).count() as f32 / total as f32;
        self.enemy_spots = ctx.spots.iter().zip(&self.enemy).filter(|(_, e)| **e).map(|(s, _)| *s).collect();
    }

    /// The next spot to claim. Below the territory target: the nearest free
    /// spot, aggressively. Above it: a slow creep — only cheap-to-hold spots
    /// (near our centroid, far from the enemy's), rate-limited to one per 30 s.
    pub fn expansion_spot(&mut self, ctx: &Ctx, ordered: &HashSet<usize>) -> Option<usize> {
        // Power distance to our centroid: squared distance minus the spot's
        // weight, so high-yield spots win even a bit farther out (the same
        // weighting gui_metal_territories uses for its cells).
        let score = |i: usize, s: &Vec3| {
            let d = s.distance_2d(self.our_centroid);
            d * d - self.weights.get(i).copied().unwrap_or(0.0)
        };
        let free = |spots: &mut dyn Iterator<Item = (usize, &Vec3)>| {
            spots
                .filter(|(i, _)| !self.owned[*i] && !self.enemy[*i] && !ordered.contains(i))
                .min_by(|(i, a), (j, b)| score(*i, a).partial_cmp(&score(*j, b)).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
        };
        if self.share * 100.0 < ctx.opts.territory_share {
            return free(&mut ctx.spots.iter().enumerate());
        }
        if ctx.frame - self.last_creep < 1800 {
            return None;
        }
        let cand = ctx
            .spots
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                !self.owned[*i]
                    && !self.enemy[*i]
                    && !ordered.contains(i)
                    && s.distance_2d(self.our_centroid) < 1500.0
                    && s.distance_2d(self.enemy_centroid) > 2000.0
            })
            .min_by(|(i, a), (j, b)| score(*i, a).partial_cmp(&score(*j, b)).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        if cand.is_some() {
            self.last_creep = ctx.frame;
        }
        cand
    }
}
