//! Cached unit-definition lookups, role classification, and the verified
//! Zero-K picks the modules order by role. Classification prefers human names
//! (what the plan is written in) with internal-name fallback; the mapping was
//! verified against the Zero-K repo (see docs/plan-v2.md round 4).

use std::collections::HashMap;

use springai::{Callback, Resource, UnitDef};

/// Engine/LuaRules command ids the typed API does not cover.
// PRIORITY/MORPH are for the engineer-taskgroup (#11) and morph-chain (#15) issues.
#[allow(dead_code)]
pub mod cmd {
    /// Zero-K's terraform gadget internal command (LuaRules `unit_terraform.lua`).
    pub const TERRAFORM_INTERNAL: i32 = 39801;
    /// Follow-up raise command the gadget expects from every builder.
    pub const RAISE: i32 = 39737;
    /// Terraform kind "raise" (relative; pyramid skirt); ditches/pits are a
    /// raise with negative height.
    pub const KIND_RAISE: f32 = 2.0;
    /// Terraform kind "level": to an absolute height, flat top, no-op where
    /// already that high (walls, platforms).
    pub const KIND_LEVEL: f32 = 1.0;
    /// ZK's priority command (economy/defense printing).
    pub const PRIORITY: i32 = 34220;
    /// ZK's morph command.
    pub const MORPH: i32 = 31210;
}

/// What a unit is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Commander,
    Worker,
    Factory,
    Scout,
    Raider,
    Skirm,
    Assault,
    Arty,
    Air,
    Other,
}

impl Role {
    /// Counts toward the army / push size (scouts do not).
    pub fn is_combat(&self) -> bool {
        matches!(self, Role::Raider | Role::Skirm | Role::Assault | Role::Arty | Role::Air)
    }
    pub fn is_scout(&self) -> bool {
        *self == Role::Scout
    }
}

/// Metal cost above which a non-flying unit counts as a "heavy" (plan round 3:
/// > 1000, not 1600; Ogre/tankriot at 500 is a riot, not a heavy).
pub const HEAVY_COST: f32 = 1000.0;
/// Morphed commanders below this cost are an edge case, not heavies.
pub const COMMANDER_HEAVY_EXEMPT_COST: f32 = 4000.0;

const SCOUTS: &[&str] = &[
    "planelightscout", // Sparrow
    "cloakaa",         // Gremlin
    "cloakheavyraid",  // Scythe
    "spiderantiheavy", // Widow
    "spiderscout",
    "vehscout",
    "shieldscout",
    "planescout",
    "shipscout",
];
const RAIDERS: &[&str] = &[
    "cloakraid",
    "shieldraid",
    "vehraid",
    "hoverraid",
    "jumpraid",
    "amphraid",
    "gunshipraid",
    "cloakheavyraid",
    "tankheavyraid",
    "subraider",
];
const SKIRMS: &[&str] = &[
    "cloakskirm",
    "cloakriot",
    "vehriot",
    "hoverriot",
    "hoverskirm",
    "shieldskirm",
    "amphriot",
    "spiderskirm",
    "spiderriot",
    "gunshipskirm",
];
const ASSAULTS: &[&str] = &[
    "tankassault",
    "tankriot", // Minotaur, Ogre
    "cloakassault",
    "vehassault",
    "hoverassault",
    "amphassault",
    "shieldassault",
    "tankheavyassault",
    "jumpsumo",
    "jumparty",
    "spiderassault",
];
const CON_NAMES: &[&str] = &[
    "cloakcon",
    "armcon",
    "corcon",
    "jumpcon",
    "amphcon",
    "hovercon",
    "shipcon",
    "shieldcon",
    "vehcon",
    "tankcon",
    "spidercon",
    "gunshipcon",
    "planecon",
];
const FACTORY_NAMES: &[&str] = &[
    "factorycloak",
    "factoryveh",
    "factoryjump",
    "factoryshield",
    "factoryhover",
    "factoryamph",
    "factoryspider",
    "factoryplane",
    "factorygunship",
];
/// Porc ladder by metal cost: Lotus 90, Stardust 220, Gauss 400, Stinger 450,
/// Desolator 1500, Lucifer 2200 (verified against the Zero-K repo).
const TURRET_LADDER: &[&str] =
    &["turretlaser", "turretriot", "turretgauss", "turretheavylaser", "turretheavy", "turretantiheavy"];
/// AA ladder: Razor 280, Hacksaw 210, Chainsaw 900, Artemis 2400.
const AA_LADDER: &[&str] = &["turretaalaser", "turretaaclose", "turretaafar", "turretaaheavy"];

/// Unit definitions indexed by internal and human name, plus the picks.
// Some picks (singu, adv_geo, pylon, aegis, aspis, silo, nuke_silo, antinuke,
// funnelweb, radar) are for the open phase-3/4 issues (#15, #16).
#[allow(dead_code)]
pub struct Defs {
    by_name: HashMap<String, UnitDef>,
    by_human: HashMap<String, UnitDef>,
    metal: Option<Resource>,
    pub mex: Option<UnitDef>,
    pub wind: Option<UnitDef>,
    pub solar: Option<UnitDef>,
    pub fusion: Option<UnitDef>,
    pub singu: Option<UnitDef>,
    pub geo: Option<UnitDef>,
    pub adv_geo: Option<UnitDef>,
    pub storage: Option<UnitDef>,
    pub pylon: Option<UnitDef>,
    pub radar: Option<UnitDef>,
    pub caretaker: Option<UnitDef>,
    pub con: Option<UnitDef>,
    pub factory: Option<UnitDef>,
    pub raider: Option<UnitDef>,
    pub skirm: Option<UnitDef>,
    pub assault: Option<UnitDef>,
    pub arty: Option<UnitDef>,
    /// Scout units, cheapest first (Sparrow/Gremlin/Scythe/Widow, ...).
    pub scouts: Vec<UnitDef>,
    pub picket: Option<UnitDef>,
    /// Light to heavy ground turrets, by metal income tier.
    pub turret_ladder: Vec<UnitDef>,
    pub aa_ladder: Vec<UnitDef>,
    pub aegis: Option<UnitDef>,
    pub aspis: Option<UnitDef>,
    pub silo: Option<UnitDef>,
    pub nuke_silo: Option<UnitDef>,
    pub antinuke: Option<UnitDef>,
    pub funnelweb: Option<UnitDef>,
}

impl Defs {
    pub fn new(cb: Callback) -> Self {
        let metal = cb.resource_by_name("Metal");
        let mut by_name = HashMap::new();
        let mut by_human = HashMap::new();
        for d in cb.unit_defs() {
            by_name.insert(d.name(), d);
            by_human.insert(d.human_name(), d);
        }
        let get = |n: &str| by_name.get(n).copied();
        let mex = get("staticmex").or_else(|| {
            by_name
                .values()
                .copied()
                .find(|d| !d.is_able_to_move() && metal.is_some_and(|m| d.extracts_resource(m) > 0.0))
        });
        let con = CON_NAMES
            .iter()
            .find_map(|n| get(n))
            .filter(|d| d.is_builder() && d.is_able_to_move() && !Self::is_commander_def(d))
            .or_else(|| {
                by_name
                    .values()
                    .copied()
                    .filter(|d| d.is_builder() && d.is_able_to_move() && !Self::is_commander_def(d))
                    .min_by(|a, b| {
                        let cost = |d: UnitDef| metal.map(|m| d.cost(m)).unwrap_or(f32::MAX);
                        cost(*a).partial_cmp(&cost(*b)).unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
        let factory =
            FACTORY_NAMES.iter().find_map(|n| get(n)).filter(|d| !d.build_options().is_empty()).or_else(|| {
                by_name.values().copied().find(|d| d.name().starts_with("factory") && !d.build_options().is_empty())
            });
        let raider = get("cloakraid").or_else(|| get("vehraid")).or_else(|| get("shieldraid"));
        let skirm = get("cloakskirm").or_else(|| get("cloakriot")).or_else(|| get("vehriot"));
        let assault = get("tankassault").or_else(|| get("shieldassault")).or_else(|| get("tankriot"));
        let arty = get("tankarty").or_else(|| get("hoverarty"));
        let mut scouts: Vec<UnitDef> = SCOUTS.iter().filter_map(|n| get(n)).collect();
        if let Some(cost) = metal {
            scouts.sort_by(|a, b| a.cost(cost).partial_cmp(&b.cost(cost)).unwrap_or(std::cmp::Ordering::Equal));
        }
        let picket = get("turretmissile");
        let turret_ladder = TURRET_LADDER.iter().filter_map(|n| get(n)).collect();
        let aa_ladder = AA_LADDER.iter().filter_map(|n| get(n)).collect();
        let aegis = get("staticshield");
        let aspis = get("shieldshield");
        let silo = get("staticmissilesilo");
        let nuke_silo = get("staticnuke");
        let antinuke = get("staticantinuke");
        let funnelweb = get("striderfunnelweb");
        let wind = get("energywind");
        let solar = get("energysolar");
        let fusion = get("energyfusion");
        let singu = get("energysingu");
        let geo = get("energygeo");
        let adv_geo = get("energyheavygeo");
        let storage = get("staticstorage");
        let pylon = get("energypylon");
        let radar = get("staticradar");
        let caretaker = get("staticcon");
        Defs {
            by_name,
            by_human,
            metal,
            mex,
            wind,
            solar,
            fusion,
            singu,
            geo,
            adv_geo,
            storage,
            pylon,
            radar,
            caretaker,
            con,
            factory,
            raider,
            skirm,
            assault,
            arty,
            scouts,
            picket,
            turret_ladder,
            aa_ladder,
            aegis,
            aspis,
            silo,
            nuke_silo,
            antinuke,
            funnelweb,
        }
    }

    pub fn get(&self, name: &str) -> Option<UnitDef> {
        self.by_name.get(name).copied()
    }

    /// Look up a definition by human (display) name, e.g. `"Lotus"`.
    pub fn by_human(&self, human: &str) -> Option<UnitDef> {
        self.by_human.get(human).copied()
    }

    pub fn metal(&self) -> Option<Resource> {
        self.metal
    }

    fn is_commander_def(def: &UnitDef) -> bool {
        let n = def.name();
        // Zero-K's commanders are the dynamic `dyn*` and `cremcom*` units.
        n.starts_with("dyn") || n.starts_with("cremcom") || n.ends_with("com")
    }

    pub fn is_commander(&self, def: UnitDef) -> bool {
        Self::is_commander_def(&def)
    }

    /// Statics worth keeping point defenses around: all factories, power
    /// plants, missile/nuke silos, anti-nukes, strider hubs.
    pub fn is_critical_static(&self, def: UnitDef) -> bool {
        if def.is_able_to_move() {
            return false;
        }
        let name = def.name();
        if name.starts_with("factory") || name == "striderhub" {
            return true;
        }
        [self.fusion, self.singu, self.geo, self.adv_geo, self.silo, self.nuke_silo, self.antinuke].contains(&Some(def))
    }

    /// Anti-air defense tower (on the AA ladder or any `turretaa*`).
    pub fn is_aa_tower(&self, def: UnitDef) -> bool {
        !def.is_able_to_move() && (self.aa_ladder.contains(&def) || def.name().contains("turretaa"))
    }

    /// A "heavy": non-flying, metal cost > HEAVY_COST; commanders under
    /// COMMANDER_HEAVY_EXEMPT_COST are an edge case and not heavies.
    pub fn is_heavy(&self, def: UnitDef) -> bool {
        if def.is_able_to_fly() {
            return false;
        }
        let Some(metal) = self.metal else { return false };
        let cost = def.cost(metal);
        if self.is_commander(def) && cost < COMMANDER_HEAVY_EXEMPT_COST {
            return false;
        }
        cost > HEAVY_COST
    }

    /// Classify a unit type for order dispatch.
    pub fn role(&self, def: UnitDef) -> Role {
        let name = def.name();
        let human = def.human_name();
        if SCOUTS.contains(&name.as_str()) || matches!(human.as_str(), "Sparrow" | "Gremlin" | "Scythe" | "Widow") {
            return Role::Scout;
        }
        if self.is_commander(def) {
            return Role::Commander;
        }
        if name.starts_with("factory") || name == "striderhub" {
            return Role::Factory;
        }
        if !def.is_able_to_move() {
            return Role::Other; // static building / turret
        }
        if def.is_builder() {
            return Role::Worker;
        }
        if def.is_able_to_fly() {
            return Role::Air;
        }
        if !def.is_able_to_attack() {
            return Role::Other;
        }
        if RAIDERS.contains(&name.as_str()) {
            return Role::Raider;
        }
        if SKIRMS.contains(&name.as_str()) {
            return Role::Skirm;
        }
        if ASSAULTS.contains(&name.as_str()) {
            return Role::Assault;
        }
        if name.contains("arty") || def.max_weapon_range() > 550.0 {
            return Role::Arty;
        }
        Role::Skirm
    }
}
