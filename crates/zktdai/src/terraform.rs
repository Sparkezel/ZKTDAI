//! Terraform helper around Zero-K's `unit_terraform.lua` gadget.
//!
//! Issue custom command 39801 (`CMD_TERRAFORM_INTERNAL`) to ONE builder with the
//! full shape (kind, team, centre, tag, area flag, height, point count, builder
//! count, volume change, then point x,z pairs, then builder ids), and 39737
//! (`CMD_RAISE`) to EVERY builder with the same tag. The gadget spawns invisible
//! `terraunit` helpers at the points; builders repair them and the heightmap is
//! written incrementally from the metal spent (`DoTerraformUpdate` ->
//! `Spring.SetHeightMap`). Digs are cancelled when enemies approach and idle
//! terraunits decay, so callers should track the tag and re-issue if needed.
//! `height < 0` digs (pit/trench); `height > 0` raises (wall/spire).

use springai::{Callback, Unit, Vec3, NO_TIMEOUT, opt};

use crate::defs::cmd;

pub fn terraform(cb: Callback, builders: &[Unit], points: &[Vec3], height: f32, tag: i32) -> bool {
    let Some(&first) = builders.first() else { return false };
    if points.len() < 2 {
        return false;
    }
    let n = points.len() as f32;
    let (cx, cz) = points.iter().fold((0.0, 0.0), |(x, z), p| (x + p.x / n, z + p.z / n));
    let mut params = vec![
        cmd::KIND_RAISE,
        cb.game().my_team() as f32,
        cx,
        cz,
        tag as f32,
        0.0, // a line, not a closed area
        height,
        n,
        builders.len() as f32,
        0.0, // any volume change
    ];
    params.extend(points.iter().flat_map(|p| [p.x, p.z]));
    params.extend(builders.iter().map(|b| b.id() as f32));
    if !first.execute_custom_command(cmd::TERRAFORM_INTERNAL, &params, opt::NONE, NO_TIMEOUT) {
        return false;
    }
    let y = cb.map().elevation_at(cx, cz);
    for b in builders {
        b.execute_custom_command(cmd::RAISE, &[cx, y, cz, tag as f32], opt::NONE, NO_TIMEOUT);
    }
    true
}

/// Points along the segment `from` -> `to`, spaced `step` elmos apart.
pub fn line_points(from: Vec3, to: Vec3, step: f32) -> Vec<Vec3> {
    let dir = to - from;
    let len = dir.length_2d().max(1.0);
    let n = (len / step).floor().max(1.0) as i32;
    (0..=n).map(|k| Vec3::xz(from.x + dir.x * k as f32 / n as f32, from.z + dir.z * k as f32 / n as f32)).collect()
}

/// Points on an arc of radius `r` around `center`, centred on the direction
/// `toward`, spanning `arc_deg` degrees total, spaced `step` elmos apart.
pub fn arc_points(center: Vec3, toward: Vec3, r: f32, arc_deg: f32, step: f32) -> Vec<Vec3> {
    let base = toward.z.atan2(toward.x); // angle of `toward` in the xz plane
    let half = arc_deg.to_radians() / 2.0;
    let arc_len = 2.0 * half * r;
    let n = (arc_len / step).floor().max(1.0) as i32;
    (0..=n)
        .map(|k| {
            let a = base - half + (2.0 * half) * k as f32 / n as f32;
            Vec3::xz(center.x + a.cos() * r, center.z + a.sin() * r)
        })
        .collect()
}
