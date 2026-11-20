//! The war chest: one ledger that owns every metal/energy spending decision.
//!
//! No other module reads `economy.current` to decide whether to spend; they ask
//! the ledger. The economy is streaming: income flows in, spending flows out, and
//! storage is the buffer that lets us sacrifice current strength for flexibility.
//! So the ledger has two gates:
//!
//! - *High-priority bucket* (mexes, radar, efficient energy, special projects):
//!   only constrained by the reserve floor — may spend
//!   `current − reserve − reservations`, i.e. draw storage down to the floor.
//! - *Normal bucket* (army production, porc): capped per 300-frame window at a
//!   fraction of metal income (`army_share`, default 60% but greedy below 10 m/s;
//!   `porc_cap_pct`), so the remaining normal share floats as reserve.
//!
//! High-priority projects additionally set ZK's construction priority
//! (cmd 34220, state 2 = High) on their builder when the build starts, so the
//! `unit_priority.lua` gadget serves that constructor from storage first while
//! normal-priority constructors are scaled down (their build speed drops by the
//! shortfall). Priority is cleared (state 1 = Normal) when the project ends.
//!
//! Plan: `docs/plan-v2.md` "war_chest ledger" (issue #4).

use std::collections::HashSet;

use springai::{NO_TIMEOUT, Resource, Unit, UnitDef, opt};

use crate::ai::Ctx;
use crate::defs::cmd;

// HeavyPorc/Silo/Nuke/Counter are for the open phase-3/4 issues (#15, #16).
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectKind {
    Army,
    Porc,
    Energy,
    Expansion,
    HeavyPorc,
    Storage,
    Silo,
    Nuke,
    Counter,
}

impl ProjectKind {
    /// High-priority bucket: mexes, radar, efficient energy, special projects.
    /// Only floor-constrained (no per-window income cap). Army and Porc are the
    /// normal bucket and get per-window caps.
    pub fn is_high_priority(self) -> bool {
        !matches!(self, ProjectKind::Army | ProjectKind::Porc)
    }

    fn tag(self) -> &'static str {
        match self {
            ProjectKind::Army => "army",
            ProjectKind::Porc => "porc",
            ProjectKind::Energy => "energy",
            ProjectKind::Expansion => "expansion",
            ProjectKind::HeavyPorc => "heavy-porc",
            ProjectKind::Storage => "storage",
            ProjectKind::Silo => "silo",
            ProjectKind::Nuke => "nuke",
            ProjectKind::Counter => "counter",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectState {
    Queued,
    Active,
    Done,
    Failed,
}

impl ProjectState {
    fn tag(self) -> &'static str {
        match self {
            ProjectState::Queued => "queued",
            ProjectState::Active => "active",
            ProjectState::Done => "done",
            ProjectState::Failed => "failed",
        }
    }
}

pub struct Project {
    pub name: &'static str,
    pub kind: ProjectKind,
    pub state: ProjectState,
    /// The def being built for this project, if any.
    pub def: Option<UnitDef>,
    /// The unit under construction.
    pub target: Option<Unit>,
    /// The builder running the project (set High priority if `priority`).
    pub builder: Option<i32>,
    /// High-priority project: its builder takes metal from storage first.
    pub priority: bool,
    /// Metal reserved (not spendable by others) while queued/active.
    pub reserved: f32,
    /// Extra porc spend allowed while active, as a fraction of the porc cap.
    pub budget_bump: f32,
    pub since: i32,
}

/// Frames in one spend window (army + porc caps are per window).
const SPEND_WINDOW: i32 = 300;
/// Below this metal income the army may take the whole normal share (greedy opening).
const EARLY_INCOME: f32 = 10.0;

// energy/reserve_energy are for the nuke project (#16).
#[allow(dead_code)]
pub struct WarChest {
    metal: Option<Resource>,
    energy: Option<Resource>,
    pub reserve_metal: f32,
    /// The reserve floor requested at game start (option `reserve_metal`).
    reserve_metal_start: f32,
    /// Storage capacity at game start; the reserve floor grows with storage
    /// above this base (0 at start = nothing reserved).
    base_storage: f32,
    pub reserve_energy: f32,
    /// Porc may spend at most this fraction of metal income per window.
    pub porc_cap_pct: f32,
    /// Army production may spend at most this fraction of metal income per
    /// window (normal bucket; greedy = 100% below EARLY_INCOME m/s).
    pub army_share: f32,
    /// Desired metal stockpile (plus headroom).
    pub stockpile_target: f32,
    pub projects: Vec<Project>,
    /// Builders we set to High construction priority (cmd 34220 state 2).
    priority_units: HashSet<i32>,
    porc_spent: f32,
    army_spent: f32,
    window_start: i32,
    /// Strategy profile chosen at game start (option `strategy`, or seeded random).
    pub strategy: String,
}

pub struct WarChestOpts {
    pub porc_cap_pct: f32,
    pub army_share: f32,
    pub stockpile_target: f32,
    pub reserve_metal_start: f32,
    pub strategy: String,
}

impl WarChest {
    pub fn new(cb: springai::Callback, opts: WarChestOpts) -> Self {
        WarChest {
            metal: cb.resource_by_name("Metal"),
            energy: cb.resource_by_name("Energy"),
            reserve_metal: opts.reserve_metal_start,
            reserve_metal_start: opts.reserve_metal_start,
            base_storage: 0.0,
            reserve_energy: 0.0,
            porc_cap_pct: opts.porc_cap_pct,
            army_share: opts.army_share,
            stockpile_target: opts.stockpile_target,
            projects: Vec::new(),
            priority_units: HashSet::new(),
            porc_spent: 0.0,
            army_spent: 0.0,
            window_start: 0,
            strategy: opts.strategy,
        }
    }

    fn current_metal(&self, ctx: &Ctx) -> f32 {
        self.metal.map(|m| ctx.cb.economy().current(m)).unwrap_or(0.0)
    }

    fn income_metal(&self, ctx: &Ctx) -> f32 {
        self.metal.map(|m| ctx.cb.economy().income(m)).unwrap_or(0.0)
    }

    fn storage_metal(&self, ctx: &Ctx) -> f32 {
        self.metal.map(|m| ctx.cb.economy().storage(m)).unwrap_or(1000.0)
    }

    /// Metal available for new spending: current minus reserve minus all
    /// reservations of queued/active projects.
    pub fn spendable(&self, ctx: &Ctx) -> f32 {
        let reserved: f32 = self
            .projects
            .iter()
            .filter(|p| p.state == ProjectState::Queued || p.state == ProjectState::Active)
            .map(|p| p.reserved)
            .sum();
        self.current_metal(ctx) - self.reserve_metal - reserved
    }

    /// Whether `amount` of metal may be spent on `kind` right now (reserve floor
    /// plus, for the normal bucket, the per-window income cap).
    pub fn can_spend(&self, ctx: &Ctx, kind: ProjectKind, amount: f32) -> bool {
        if amount > self.spendable(ctx) {
            return false;
        }
        if !kind.is_high_priority()
            && let Some((spent, cap)) = self.window_cap(ctx, kind)
            && spent + amount > cap.max(1.0)
        {
            return false;
        }
        true
    }

    /// Per-window income cap for the normal bucket (army, porc); `None` for the
    /// high-priority bucket, which is only floor-constrained. The army is greedy
    /// in the opening: below `EARLY_INCOME` m/s it may take the whole normal share.
    fn window_cap(&self, ctx: &Ctx, kind: ProjectKind) -> Option<(f32, f32)> {
        let income = self.income_metal(ctx);
        let window = SPEND_WINDOW as f32 / 30.0;
        match kind {
            ProjectKind::Porc => Some((self.porc_spent, self.porc_cap_pct * income * window)),
            ProjectKind::Army => {
                let share = if income < EARLY_INCOME { 1.0 } else { self.army_share };
                Some((self.army_spent, share * income * window))
            }
            _ => None,
        }
    }

    /// Reserve metal for a new project. Returns false if the ledger can't afford it.
    pub fn request(
        &mut self,
        ctx: &Ctx,
        name: &'static str,
        kind: ProjectKind,
        def: Option<UnitDef>,
        cost: f32,
        budget_bump: f32,
    ) -> bool {
        self.request_inner(ctx, name, kind, def, cost, budget_bump, false)
    }

    /// Same, but the project's builder gets High construction priority (cmd
    /// 34220 state 2) when the build starts: it draws metal from storage first
    /// while normal-priority constructors are scaled down by the gadget.
    pub fn request_priority(
        &mut self,
        ctx: &Ctx,
        name: &'static str,
        kind: ProjectKind,
        def: Option<UnitDef>,
        cost: f32,
        budget_bump: f32,
    ) -> bool {
        self.request_inner(ctx, name, kind, def, cost, budget_bump, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn request_inner(
        &mut self,
        ctx: &Ctx,
        name: &'static str,
        kind: ProjectKind,
        def: Option<UnitDef>,
        cost: f32,
        budget_bump: f32,
        priority: bool,
    ) -> bool {
        if !self.can_spend(ctx, kind, cost) {
            return false;
        }
        self.projects.push(Project {
            name,
            kind,
            state: ProjectState::Queued,
            def,
            target: None,
            builder: None,
            priority,
            reserved: cost,
            budget_bump,
            since: ctx.frame,
        });
        true
    }

    /// Set a builder to High construction priority (cmd 34220, state 2 = High).
    fn set_high_priority(&mut self, cb: springai::Callback, builder: Unit) {
        if self.priority_units.insert(builder.id()) {
            builder.execute_custom_command(cmd::PRIORITY, &[2.0], opt::NONE, NO_TIMEOUT);
            cb.log(&format!("[war_chest] builder {} set High priority", builder.id()));
        }
    }

    /// Back to Normal priority (state 1) for a builder we flagged.
    fn release_priority(&mut self, cb: springai::Callback, builder_id: i32) {
        if self.priority_units.remove(&builder_id)
            && let Some(b) = Unit::from_id(cb, builder_id)
        {
            b.execute_custom_command(cmd::PRIORITY, &[1.0], opt::NONE, NO_TIMEOUT);
        }
    }

    /// A builder of ours started building `unit`: activate the matching project.
    pub fn on_unit_created(&mut self, ctx: &Ctx, unit: Unit, builder: Option<Unit>) {
        let Some(def) = unit.def() else { return };
        let activate = self.projects.iter().position(|p| p.state == ProjectState::Queued && p.def == Some(def));
        let Some(i) = activate else { return };
        let (name, priority) = {
            let p = &mut self.projects[i];
            p.state = ProjectState::Active;
            p.target = Some(unit);
            p.builder = builder.map(|b| b.id());
            (p.name, p.priority)
        };
        ctx.cb.log(&format!("[war_chest] project {} active (frame {})", name, ctx.frame));
        if priority && let Some(b) = builder {
            self.set_high_priority(ctx.cb, b);
        }
    }

    pub fn on_unit_finished(&mut self, ctx: &Ctx, unit: Unit) {
        let mut releases = Vec::new();
        for p in &mut self.projects {
            if p.target == Some(unit) && p.state == ProjectState::Active {
                p.state = ProjectState::Done;
                p.reserved = 0.0;
                if let Some(bid) = p.builder {
                    releases.push(bid);
                }
                ctx.cb.log(&format!("[war_chest] project {} done (frame {})", p.name, ctx.frame));
            }
        }
        for bid in releases {
            self.release_priority(ctx.cb, bid);
        }
    }

    pub fn on_unit_destroyed(&mut self, ctx: &Ctx, unit: Unit) {
        let mut releases = Vec::new();
        for p in &mut self.projects {
            if p.target == Some(unit) && p.state != ProjectState::Done {
                p.state = ProjectState::Failed;
                p.reserved = 0.0;
                if let Some(bid) = p.builder {
                    releases.push(bid);
                }
                ctx.cb.log(&format!("[war_chest] project {} failed (frame {})", p.name, ctx.frame));
            }
        }
        for bid in releases {
            self.release_priority(ctx.cb, bid);
        }
    }

    /// Per-window bookkeeping and the reserve floor; call every ~30 frames.
    pub fn update(&mut self, ctx: &Ctx) {
        // Reserve floor: nothing reserved at game start; grows as 10% of storage
        // capacity above the starting base, at least the requested start value,
        // never more than the stockpile target. This is the hard limit for burst
        // spending: storage may be drawn down to it.
        let storage = self.storage_metal(ctx);
        if self.base_storage <= 0.0 {
            self.base_storage = storage;
        }
        let lo = self.reserve_metal_start.min(self.stockpile_target); // clamp panics if lo > hi
        self.reserve_metal = (0.1 * (storage - self.base_storage).max(0.0)).clamp(lo, self.stockpile_target);
        // Drop priority flags of builders that died.
        self.priority_units.retain(|id| Unit::from_id(ctx.cb, *id).is_some());
        if ctx.frame - self.window_start >= SPEND_WINDOW {
            self.window_start = ctx.frame;
            self.porc_spent = 0.0;
            self.army_spent = 0.0;
        }
        // A queued project whose build never started (no free builder) releases
        // its reservation after 100 s instead of locking the metal forever.
        for p in &mut self.projects {
            if p.state == ProjectState::Queued && ctx.frame - p.since > 3000 {
                p.state = ProjectState::Failed;
                p.reserved = 0.0;
                ctx.cb.log(&format!("[war_chest] project {} never started, released", p.name));
            }
        }
        // Drop finished/failed projects from the list after an hour.
        self.projects
            .retain(|p| !(matches!(p.state, ProjectState::Done | ProjectState::Failed) && ctx.frame - p.since > 36000));
        if ctx.frame % 900 == 0 {
            ctx.cb.log(&format!(
                "[war_chest] frame={} metal={:.0} income={:.1} reserve={:.0} spendable={:.0} porc_spent={:.0} army_spent={:.0} strategy={} projects=[{}]",
                ctx.frame,
                self.current_metal(ctx),
                self.income_metal(ctx),
                self.reserve_metal,
                self.spendable(ctx),
                self.porc_spent,
                self.army_spent,
                self.strategy,
                self.projects.iter().map(|p| format!("{}:{}", p.name, p.state.tag())).collect::<Vec<_>>().join(",")
            ));
        }
    }

    /// Note normal-bucket spending so the per-window cap holds.
    pub fn note_spend(&mut self, kind: ProjectKind, amount: f32) {
        match kind {
            ProjectKind::Porc => self.porc_spent += amount,
            ProjectKind::Army => self.army_spent += amount,
            _ => {}
        }
    }

    /// JSON status for the operator widget / infolog (issue #14 wires transport).
    pub fn status_json(&self, ctx: &Ctx) -> String {
        let projects: Vec<String> = self
            .projects
            .iter()
            .map(|p| {
                format!(
                    "{{\"name\":\"{}\",\"kind\":\"{}\",\"state\":\"{}\",\"reserved\":{:.0},\"bump\":{:.2}}}",
                    p.name,
                    p.kind.tag(),
                    p.state.tag(),
                    p.reserved,
                    p.budget_bump
                )
            })
            .collect();
        format!(
            "{{\"frame\":{},\"metal\":{:.0},\"income\":{:.1},\"reserve\":{:.0},\"spendable\":{:.0},\"porc_cap\":{:.2},\"army_share\":{:.2},\"army_spent\":{:.0},\"porc_spent\":{:.0},\"stockpile\":{:.0},\"strategy\":\"{}\",\"projects\":[{}]}}",
            ctx.frame,
            self.current_metal(ctx),
            self.income_metal(ctx),
            self.reserve_metal,
            self.spendable(ctx),
            self.porc_cap_pct,
            self.army_share,
            self.army_spent,
            self.porc_spent,
            self.stockpile_target,
            self.strategy,
            projects.join(",")
        )
    }
}
