//! The war chest: one ledger that owns every metal/energy spending decision.
//!
//! No other module reads `economy.current` to decide whether to spend; they ask
//! the ledger. State: a reserve floor (metal kept unspent, growing with storage),
//! an energy reserve, a porc spend cap (fraction of income per window), a
//! stockpile target, and the strategic project list with its lifecycle
//! (Queued/Active/Done/Failed — reservations and budget bumps are released when
//! the project's build finishes, is destroyed, or its command is cancelled).
//!
//! Plan: `docs/plan-v2.md` "war_chest ledger" (issue #4).

use springai::{Resource, Unit, UnitDef};

use crate::ai::Ctx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectKind {
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
    fn tag(self) -> &'static str {
        match self {
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
    /// Metal reserved (not spendable by others) while queued/active.
    pub reserved: f32,
    /// Extra porc spend allowed while active, as a fraction of the porc cap.
    pub budget_bump: f32,
    pub since: i32,
}

/// Frames in one porc spend window.
const PORC_WINDOW: i32 = 300;

pub struct WarChest {
    metal: Option<Resource>,
    energy: Option<Resource>,
    pub reserve_metal: f32,
    pub reserve_energy: f32,
    /// Porc may spend at most this fraction of metal income per window.
    pub porc_cap_pct: f32,
    /// Desired metal stockpile (plus headroom).
    pub stockpile_target: f32,
    pub projects: Vec<Project>,
    porc_spent: f32,
    porc_window_start: i32,
    /// Strategy profile chosen at game start (option `strategy`, or seeded random).
    pub strategy: String,
}

pub struct WarChestOpts {
    pub porc_cap_pct: f32,
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
            reserve_energy: 0.0,
            porc_cap_pct: opts.porc_cap_pct,
            stockpile_target: opts.stockpile_target,
            projects: Vec::new(),
            porc_spent: 0.0,
            porc_window_start: 0,
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
    /// plus, for porc, the per-window income cap).
    pub fn can_spend(&self, ctx: &Ctx, kind: ProjectKind, amount: f32) -> bool {
        if amount > self.spendable(ctx) {
            return false;
        }
        if kind == ProjectKind::Porc {
            let cap = self.porc_cap_pct * self.income_metal(ctx) * PORC_WINDOW as f32 / 30.0;
            if self.porc_spent + amount > cap.max(1.0) {
                return false;
            }
        }
        true
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
        if !self.can_spend(ctx, kind, cost) {
            return false;
        }
        self.projects.push(Project {
            name,
            kind,
            state: ProjectState::Queued,
            def,
            target: None,
            reserved: cost,
            budget_bump,
            since: ctx.frame,
        });
        true
    }

    /// A builder of ours started building `unit`: activate the matching project.
    pub fn on_unit_created(&mut self, ctx: &Ctx, unit: Unit, builder: Option<Unit>) {
        if builder.is_none() {
            return;
        }
        let Some(def) = unit.def() else { return };
        for p in &mut self.projects {
            if p.state == ProjectState::Queued && p.def == Some(def) {
                p.state = ProjectState::Active;
                p.target = Some(unit);
                ctx.cb.log(&format!("[war_chest] project {} active (frame {})", p.name, ctx.frame));
                return;
            }
        }
    }

    pub fn on_unit_finished(&mut self, ctx: &Ctx, unit: Unit) {
        for p in &mut self.projects {
            if p.target == Some(unit) && p.state == ProjectState::Active {
                p.state = ProjectState::Done;
                p.reserved = 0.0;
                ctx.cb.log(&format!("[war_chest] project {} done (frame {})", p.name, ctx.frame));
            }
        }
    }

    pub fn on_unit_destroyed(&mut self, ctx: &Ctx, unit: Unit) {
        for p in &mut self.projects {
            if p.target == Some(unit) && p.state != ProjectState::Done {
                p.state = ProjectState::Failed;
                p.reserved = 0.0;
                ctx.cb.log(&format!("[war_chest] project {} failed (frame {})", p.name, ctx.frame));
            }
        }
    }

    /// Per-window bookkeeping and the reserve floor; call every ~30 frames.
    pub fn update(&mut self, ctx: &Ctx) {
        // Reserve floor: ~10% of storage capacity, at least the starting reserve,
        // never more than the stockpile target.
        let storage = self.storage_metal(ctx);
        self.reserve_metal =
            (0.1 * storage).clamp(self.reserve_metal.min(50.0), self.stockpile_target);
        if ctx.frame - self.porc_window_start >= PORC_WINDOW {
            self.porc_window_start = ctx.frame;
            self.porc_spent = 0.0;
        }
        // Drop finished/failed projects from the list after an hour.
        self.projects
            .retain(|p| !(matches!(p.state, ProjectState::Done | ProjectState::Failed) && ctx.frame - p.since > 36000));
        if ctx.frame % 900 == 0 {
            ctx.cb.log(&format!(
                "[war_chest] frame={} metal={:.0} income={:.1} reserve={:.0} spendable={:.0} strategy={} projects=[{}]",
                ctx.frame,
                self.current_metal(ctx),
                self.income_metal(ctx),
                self.reserve_metal,
                self.spendable(ctx),
                self.strategy,
                self.projects
                    .iter()
                    .map(|p| format!("{}:{}", p.name, p.state.tag()))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    }

    /// Note porc spending so the per-window cap holds.
    pub fn note_porc_spend(&mut self, amount: f32) {
        self.porc_spent += amount;
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
            "{{\"frame\":{},\"metal\":{:.0},\"income\":{:.1},\"reserve\":{:.0},\"spendable\":{:.0},\"porc_cap\":{:.2},\"stockpile\":{:.0},\"strategy\":\"{}\",\"projects\":[{}]}}",
            ctx.frame,
            self.current_metal(ctx),
            self.income_metal(ctx),
            self.reserve_metal,
            self.spendable(ctx),
            self.porc_cap_pct,
            self.stockpile_target,
            self.strategy,
            projects.join(",")
        )
    }
}
