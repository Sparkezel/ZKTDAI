//! Cached unit-definition lookups and Zero-K role classification.

use std::collections::HashMap;

use springai::{Callback, UnitDef};

/// Engine/LuaRules command ids the typed API does not cover.
pub mod cmd {
    /// Zero-K's terraform gadget internal command (LuaRules `unit_terraform.lua`).
    pub const TERRAFORM_INTERNAL: i32 = 39801;
    /// Follow-up raise command the gadget expects from every builder.
    pub const RAISE: i32 = 39737;
    /// Terraform kind "raise"; ditches are a raise with negative height.
    pub const KIND_RAISE: f32 = 2.0;
}

/// What a unit is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Commander,
    Worker,
    Factory,
    Raider,
    Skirm,
    Assault,
    Arty,
    Air,
    Support,
    Other,
}

impl Role {
    pub fn is_combat(&self) -> bool {
        matches!(self, Role::Raider | Role::Skirm | Role::Assault | Role::Arty | Role::Air)
    }
}

const RAIDERS: &[&str] = &[
    "cloakraid", "shieldraid", "vehraid", "hoverraid", "jumpraid", "amphraid", "gunshipraid",
    "cloakheavyraid", "tankheavyraid", "subraider", "shieldscout", "vehscout", "spiderscout",
    "shipscout", "shiptorpraider", "amphbomb",
];
const SKIRMS: &[&str] = &[
    "cloakskirm", "cloakriot", "tankriot", "vehriot", "hoverriot", "shieldskirm", "amphriot",
    "spiderskirm", "spiderriot", "hoverskirm", "gunshipskirm", "amphfloater", "amphimpulse",
];
const ASSAULTS: &[&str] = &[
    "tankassault", "cloakassault", "vehassault", "hoverassault", "amphassault", "shieldassault",
    "tankheavyassault", "jumpsumo", "jumparty", "spiderassault", "hoverdepthcharge", "spideremp",
];
const CON_NAMES: &[&str] = &[
    "cloakcon", "armcon", "corcon", "jumpcon", "amphcon", "hovercon", "shipcon", "shieldcon",
    "vehcon", "tankcon", "spidercon", "gunshipcon", "planecon",
];
const FACTORY_NAMES: &[&str] = &[
    "factorycloak", "factoryveh", "factoryjump", "factoryshield", "factoryhover", "factoryamph",
    "factoryspider", "factoryplane", "factorygunship",
];
const TURRET_LADDER: &[&str] = &["turretlaser", "turretriot", "turretgauss", "turretheavylaser"];

/// Unit definitions indexed by name, built once at startup, plus the
/// Zero-K-specific picks the modules order by role.
pub struct Defs {
    by_name: HashMap<String, UnitDef>,
    pub mex: Option<UnitDef>,
    pub solar: Option<UnitDef>,
    pub con: Option<UnitDef>,
    pub factory: Option<UnitDef>,
    pub raider: Option<UnitDef>,
    pub skirm: Option<UnitDef>,
    pub assault: Option<UnitDef>,
    pub arty: Option<UnitDef>,
    /// Light to heavy ground turrets, by metal income tier.
    pub turret_ladder: Vec<UnitDef>,
    pub aa: Option<UnitDef>,
}

impl Defs {
    pub fn new(cb: Callback) -> Self {
        let metal = cb.resource_by_name("Metal");
        let energy = cb.resource_by_name("Energy");
        let mut by_name = HashMap::new();
        for d in cb.unit_defs() {
            by_name.insert(d.name(), d);
        }
        let get = |n: &str| by_name.get(n).copied();
        let mex = get("staticmex").or_else(|| {
            by_name.values().copied().find(|d| {
                !d.is_able_to_move() && metal.is_some_and(|m| d.extracts_resource(m) > 0.0)
            })
        });
        let solar = get("energysolar").or_else(|| {
            by_name.values().copied().find(|d| {
                !d.is_able_to_move() && energy.is_some_and(|e| d.makes_resource(e) > 0.0)
            })
        });
        let con = CON_NAMES
            .iter()
            .find_map(|n| get(n))
            .filter(|d| d.is_builder() && d.is_able_to_move() && !Self::is_commander_def(d))
            .or_else(|| {
                by_name
                    .values()
                    .copied()
                    .filter(|d| {
                        d.is_builder() && d.is_able_to_move() && !Self::is_commander_def(d)
                    })
                    .min_by(|a, b| {
                        let cost = |d: UnitDef| metal.map(|m| d.cost(m)).unwrap_or(f32::MAX);
                        cost(*a).partial_cmp(&cost(*b)).unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
        let factory = FACTORY_NAMES
            .iter()
            .find_map(|n| get(n))
            .filter(|d| !d.build_options().is_empty())
            .or_else(|| {
                by_name.values().copied().find(|d| d.name().starts_with("factory") && !d.build_options().is_empty())
            });
        let raider = get("cloakraid").or_else(|| get("shieldraid")).or_else(|| get("vehraid"));
        let skirm = get("cloakskirm").or_else(|| get("cloakriot")).or_else(|| get("tankriot"));
        let assault = get("tankassault").or_else(|| get("cloakassault")).or_else(|| get("vehassault"));
        let arty = get("tankarty").or_else(|| get("hoverarty"));
        let turret_ladder = TURRET_LADDER.iter().filter_map(|n| get(n)).collect();
        let aa = get("turretaalaser");
        Defs { by_name, mex, solar, con, factory, raider, skirm, assault, arty, turret_ladder, aa }
    }

    pub fn get(&self, name: &str) -> Option<UnitDef> {
        self.by_name.get(name).copied()
    }

    fn is_commander_def(def: &UnitDef) -> bool {
        let n = def.name();
        // Zero-K's commanders are the dynamic `dyn*` units.
        n.starts_with("dyn") || n.ends_with("com")
    }

    pub fn is_commander(&self, def: UnitDef) -> bool {
        Self::is_commander_def(&def)
    }

    /// Classify a unit type for order dispatch.
    pub fn role(&self, def: UnitDef) -> Role {
        let name = def.name();
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
            return Role::Support;
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
