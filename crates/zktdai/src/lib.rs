//! ZKTDAI — a Zero-K skirmish AI in Rust, derived from Anarchid's zkgbai-rs
//! (the full upstream source sits in `reference/zkgbai-rs/`, git-ignored).
//!
//! Doctrine (see `docs/design.md`): offensive-defense. Hold about half the
//! map, box the enemy in with raider detachments, porc the perimeter with
//! escalating turrets and terraform ditches, and hit undefended enemy
//! critical targets with surgical strikes. Offense runs on riot-skirm
//! armies, assault armies, raider detachments and artillery groups.

mod ai;
mod army;
mod defs;
mod economy;
mod fob;
mod fortify;
mod porc;
mod sabotage;
mod terraform;
mod territory;
mod war_chest;

pub use ai::Zktdai;

springai::export_ai!(ai::Zktdai);
