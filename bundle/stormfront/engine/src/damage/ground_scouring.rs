// Ground scouring: what sustained near-surface wind does to the ground.
//
// Each column of soil accumulates an erosion dose, the time integral of the square of the wind
// excess over a threshold, weighted by how erodible its surface is (loose sand and bare dirt
// more than turf, which roots bind; wet soil less than dry). Doses past successive thresholds
// strip the turf (grass to dirt, in irregular patches), take the topsoil (coarse dirt, exposed
// roots), then dig the column down. The deepest stage needs winds above ~80 m/s held for a long
// time, which happens mostly under slow-moving violent tornadoes and along the paths of
// suction vortices: narrow corridors and cycloidal marks rather than a uniform swath. Digging
// is capped a few blocks below the original ground.

use super::materials::Kind;
use super::world::VoxelWindow;
use crate::math::hash01;

/// Stages and their wind thresholds (m/s) and doses.
pub const STRIP_TURF: (f64, f64) = (48.0, 1.5);
pub const TOPSOIL: (f64, f64) = (62.0, 3.0);
pub const DIG: (f64, f64) = (74.0, 7.0);
pub const TRENCH: (f64, f64) = (85.0, 12.0);
/// Deepest the scouring digs below the original ground (blocks).
pub const MAX_DEPTH: i32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScourState {
    pub dose: [f64; 4],
    /// Deepest stage reached (0 none .. 4 trench).
    pub stage: u8,
}

/// How easily the surface block erodes (0 never).
pub fn erodibility(state: &str, kind: Kind) -> f64 {
    let name = state.split('[').next().unwrap_or(state);
    let name = name.rsplit(':').next().unwrap_or(name);
    match kind {
        Kind::Soil => match name {
            "grass_block" | "podzol" | "mycelium" | "moss_block" | "pale_moss_block" => 0.6,
            "dirt" | "farmland" | "dirt_path" => 1.0,
            "coarse_dirt" | "rooted_dirt" => 0.85,
            "mud" | "clay" | "muddy_mangrove_roots" => 0.35,
            _ => 0.7,
        },
        Kind::Sand => 1.6,
        Kind::Gravel => 0.5,
        Kind::Snow => 2.0,
        _ => 0.0,
    }
}

/// A block edit scouring asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum ScourEdit {
    Replace([i32; 3], &'static str),
    Remove([i32; 3]),
}

/// Advance the scouring of column (x, z) by `dt` seconds of near-surface wind `u` (m/s) on
/// ground of wetness `wet` (0..1). Returns the edits and the stage reached this step (if new).
pub fn step_column(w: &VoxelWindow, st: &mut ScourState, x: i32, z: i32, u: f64, wet: f64, dt: f64) -> (Vec<ScourEdit>, Option<u8>) {
    let mut edits = Vec::new();
    let Some(c) = w.column(x, z) else { return (edits, None) };
    let g = c.ground;
    let original = c.original_ground;
    let top = [x, g, z];
    let Some(state) = w.state(top) else { return (edits, None) };
    let Some(m) = w.material(top) else { return (edits, None) };
    // something built or growing on top shields the soil
    let cover = w.material([x, g + 1, z]).is_some_and(|m| m.is_solid());
    let e = erodibility(state, m.kind) * (1.0 - 0.45 * wet.clamp(0.0, 1.0));
    if e <= 0.0 || cover {
        return (edits, None);
    }
    // irregular patches: the local resistance varies from spot to spot
    let patch = 0.65 + 0.7 * hash01(x, 0, z, 0x5C0);
    let stages = [STRIP_TURF, TOPSOIL, DIG, TRENCH];
    for (i, (threshold, _)) in stages.iter().enumerate() {
        let excess = (u - threshold * patch.sqrt()).max(0.0);
        st.dose[i] += dt * e * excess * excess / 100.0;
    }
    let reached = (0..4).rev().find(|&i| st.dose[i] >= stages[i].1 * patch).map(|i| i as u8 + 1).unwrap_or(0);
    if reached <= st.stage {
        return (edits, None);
    }
    let name = state.split('[').next().unwrap_or(state).rsplit(':').next().unwrap_or("");
    match reached {
        1 => {
            if matches!(name, "grass_block" | "podzol" | "mycelium" | "moss_block" | "pale_moss_block" | "farmland" | "dirt_path") {
                edits.push(ScourEdit::Replace(top, "minecraft:dirt"));
            }
        }
        2 => {
            if m.kind == Kind::Soil && name != "coarse_dirt" && name != "rooted_dirt" {
                // exposed roots here and there
                let roots = hash01(x, g, z, 0x2007) < 0.25;
                edits.push(ScourEdit::Replace(top, if roots { "minecraft:rooted_dirt" } else { "minecraft:coarse_dirt" }));
            }
        }
        _ => {
            let depth_goal = if reached == 3 { 1 } else { MAX_DEPTH };
            let floor = original - depth_goal;
            if g > floor {
                edits.push(ScourEdit::Remove(top));
                // the newly exposed soil is raw
                let below = [x, g - 1, z];
                if w.material(below).is_some_and(|m| m.kind == Kind::Soil) {
                    edits.push(ScourEdit::Replace(below, "minecraft:coarse_dirt"));
                }
                // dig again only after a further dose
                st.dose[(reached - 1) as usize] *= 0.45;
                return (edits, Some(reached));
            }
        }
    }
    st.stage = reached;
    (edits, Some(reached))
}

#[cfg(test)]
mod tests {
    use super::super::world::testing::MemoryWorld;
    use super::super::world::{READ_ABOVE, READ_BELOW, WorldIo};
    use super::*;

    fn window() -> VoxelWindow {
        let mut world = MemoryWorld::flat(64);
        let mut w = VoxelWindow::new();
        let cols: Vec<(i32, i32)> = (0..20).flat_map(|x| (0..20).map(move |z| (x, z))).collect();
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 0.0);
        w
    }

    fn apply(w: &mut VoxelWindow, edits: &[ScourEdit]) {
        for e in edits {
            match e {
                ScourEdit::Replace(p, s) => {
                    w.set_state(*p, s);
                }
                ScourEdit::Remove(p) => {
                    w.remove(*p);
                }
            }
            let p = match e {
                ScourEdit::Replace(p, _) | ScourEdit::Remove(p) => *p,
            };
            w.refresh_ground(p[0], p[2]);
        }
    }

    #[test]
    fn moderate_wind_strips_turf_but_does_not_dig() {
        let mut w = window();
        let mut stripped = 0;
        for x in 0..20 {
            for z in 0..20 {
                let mut st = ScourState::default();
                for _ in 0..30 {
                    let (e, _) = step_column(&w, &mut st, x, z, 58.0, 0.0, 1.0);
                    apply(&mut w, &e);
                }
                if w.state([x, 64, z]) == Some("minecraft:dirt") {
                    stripped += 1;
                }
                assert!(w.column(x, z).unwrap().ground >= 64);
            }
        }
        // patchy, not uniform
        assert!(stripped > 40 && stripped < 400, "{stripped}");
    }

    #[test]
    fn extreme_sustained_wind_digs_but_not_past_the_cap() {
        let mut w = window();
        let mut st = ScourState::default();
        for _ in 0..600 {
            let (e, _) = step_column(&w, &mut st, 5, 5, 100.0, 0.0, 1.0);
            apply(&mut w, &e);
        }
        let g = w.column(5, 5).unwrap().ground;
        assert!(g < 64, "dug: {g}");
        assert!(g >= 64 - MAX_DEPTH, "capped: {g}");
    }

    #[test]
    fn brief_violent_gusts_barely_scar_the_ground() {
        let mut w = window();
        let mut st = ScourState::default();
        let (e, _) = step_column(&w, &mut st, 3, 3, 95.0, 0.0, 0.3);
        apply(&mut w, &e);
        assert_eq!(w.column(3, 3).unwrap().ground, 64);
    }
}
