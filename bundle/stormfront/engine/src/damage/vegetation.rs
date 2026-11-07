// Trees in the wind.
//
// Leaves go block by block through the generic load model (defoliation proceeds from the
// windward side inward). Trunks are handled as whole trees: the crown's sail area times the
// dynamic pressure at crown height bends the trunk; past the species' critical wind it snaps at
// a weak point (anywhere from low to mid trunk) or, in wet soil with shallow roots, uproots. A
// defoliated tree has less sail and survives longer. A felled trunk is laid on the ground in the
// direction of the wind that felled it (as surveyors read tree-fall patterns); in extreme winds
// it is torn into flying debris instead. Trunks that remain standing in extreme winds laden with
// debris lose their bark in patches (stripped log blocks), more where wind and abrasion were
// strongest.

use super::materials::{Kind, Species};
use super::world::VoxelWindow;
use crate::math::{Rng, Vec3, hash01};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TreeState {
    /// Leaf blocks counted the first time the tree was seen.
    pub initial_leaves: u32,
    pub fatigue: f64,
    pub gone: bool,
}

/// A standing trunk found in a column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trunk {
    /// The lowest log (resting on soil).
    pub base: [i32; 3],
    pub height: i32,
    pub species: Species,
    /// Several trunks side by side (dark oak, big jungle and spruce trees) are much stiffer.
    pub thick: bool,
}

/// What happened to a tree this step.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeOutcome {
    Standing,
    /// Snapped at `at` (the first removed log); the logs above go with the crown.
    Snapped { at: [i32; 3] },
    Uprooted,
}

/// The trunk standing in column (x, z), if its lowest log rests on the column's ground.
pub fn find_trunk(w: &VoxelWindow, x: i32, z: i32) -> Option<Trunk> {
    let c = w.column(x, z)?;
    let base_y = c.ground + 1;
    let base = [x, base_y, z];
    let m = w.material(base)?;
    if m.kind != Kind::Log && m.kind != Kind::StrippedLog {
        return None;
    }
    let state = w.state(base)?;
    if !state.contains("axis=y") && state.contains("axis=") {
        return None;
    }
    let mut height = 0;
    while w.material([x, base_y + height, z]).is_some_and(|m| matches!(m.kind, Kind::Log | Kind::StrippedLog)) {
        height += 1;
        if height > 40 {
            break;
        }
    }
    // a trunk has leaves near its top; a pillar of logs in a building does not
    let top = base_y + height;
    let leafy = (-2..=2).any(|dx| (-1..=2).any(|dy| (-2..=2).any(|dz| w.material([x + dx, top + dy - 1, z + dz]).is_some_and(|m| m.kind == Kind::Leaves))));
    if !leafy && height < 3 {
        return None;
    }
    let thick = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| w.material([x + dx, base_y, z + dz]).is_some_and(|m| m.kind == Kind::Log));
    Some(Trunk { base, height, species: Species::of(state), thick })
}

/// Leaves within the crown's reach of a trunk.
pub fn count_leaves(w: &VoxelWindow, t: &Trunk) -> u32 {
    let top = t.base[1] + t.height;
    let mut n = 0;
    for dx in -3..=3 {
        for dz in -3..=3 {
            for y in (top - 4)..=(top + 3) {
                if w.material([t.base[0] + dx, y, t.base[2] + dz]).is_some_and(|m| m.kind == Kind::Leaves) {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Crown wind (m/s) that breaks this tree now, given the fraction of its crown left.
pub fn critical_wind(t: &Trunk, leaf_fraction: f64) -> f64 {
    let (base, _) = t.species.critical_wind();
    let thick = if t.thick { 1.35 } else { 1.0 };
    // taller trees carry their sail higher: a larger moment for the same wind
    let height = (6.0 / t.height.max(3) as f64).powf(0.25);
    let quality = 0.82 + 0.4 * hash01(t.base[0], t.base[1], t.base[2], 0x7EE);
    // drag goes with the sail area, so the critical wind with 1/sqrt(area)
    let sail = 0.25 + 0.75 * leaf_fraction.clamp(0.0, 1.0);
    base * thick * height * quality / sail.sqrt()
}

/// Decide the fate of a tree in a crown wind `u` for `dt` seconds.
pub fn step_tree(state: &mut TreeState, t: &Trunk, leaves_now: u32, u: f64, wet: f64, dt: f64, rng: &mut Rng) -> TreeOutcome {
    if state.initial_leaves == 0 {
        state.initial_leaves = leaves_now.max(1);
    }
    let frac = leaves_now as f64 / state.initial_leaves.max(1) as f64;
    let crit = critical_wind(t, frac);
    let ratio = u / crit;
    if ratio > 0.8 {
        state.fatigue += dt * 0.1 * (ratio - 0.8) / 0.2;
    }
    let effective = crit * (1.0 - 0.25 * state.fatigue.min(1.0));
    if u < effective {
        return TreeOutcome::Standing;
    }
    let (_, shallow) = t.species.critical_wind();
    let uproot_p = if shallow { 0.35 + 0.5 * wet } else { 0.1 + 0.35 * wet };
    if rng.chance(uproot_p) {
        TreeOutcome::Uprooted
    } else {
        // break at a weak point: knots, decay, the base of the crown
        let lo = (t.height as f64 * 0.12).floor() as i32;
        let hi = ((t.height as f64 * 0.6).ceil() as i32).max(lo + 1);
        let k = lo + rng.below((hi - lo).max(1) as u32) as i32;
        TreeOutcome::Snapped { at: [t.base[0], t.base[1] + k.min(t.height - 1), t.base[2]] }
    }
}

/// Blocks to lay down for a felled trunk of `length` logs from `from` along `wind`, as
/// (position, state): logs with their axis along the fall, on top of the ground.
pub fn felled_trunk(w: &VoxelWindow, from: [i32; 3], length: i32, species: Species, stripped: bool, wind: Vec3) -> Vec<([i32; 3], String)> {
    let h = wind.horizontal();
    let (dx, dz, axis) = if h.x.abs() >= h.z.abs() {
        (h.x.signum() as i32, 0, 'x')
    } else {
        (0, h.z.signum() as i32, 'z')
    };
    let prefix = if stripped { "stripped_" } else { "" };
    let state = format!("minecraft:{prefix}{}_log[axis={axis}]", species.prefix());
    let mut out = Vec::new();
    for k in 1..=length.min(12) {
        let x = from[0] + dx * k;
        let z = from[2] + dz * k;
        let Some(c) = w.column(x, z) else { break };
        // lie on whatever is on the ground there
        let mut y = c.ground + 1;
        while w.material([x, y, z]).is_some_and(|m| m.is_solid()) && y < c.ground + 6 {
            y += 1;
        }
        if w.material([x, y, z]).is_none_or(|m| m.is_solid()) {
            break;
        }
        out.push(([x, y, z], state.clone()));
    }
    out
}

/// The debarked form of a log state (`oak_log[axis=y]` -> `stripped_oak_log[axis=y]`).
pub fn stripped(state: &str) -> Option<String> {
    let (ns, rest) = state.split_once(':').unwrap_or(("minecraft", state));
    if rest.starts_with("stripped_") {
        return None;
    }
    let name = rest.split('[').next().unwrap_or(rest);
    let ok = name.ends_with("_log") || name.ends_with("_wood") || name.ends_with("_stem") || name.ends_with("_hyphae");
    ok.then(|| format!("{ns}:stripped_{rest}"))
}

/// Chance per second that an exposed log block loses its bark, in wind `u` (m/s) with
/// `abrasion` (debris hits nearby, 0..). Debarking is an EF4-EF5 indicator: nothing below
/// ~62 m/s without heavy debris.
pub fn debark_rate(u: f64, abrasion: f64) -> f64 {
    let wind = ((u - 62.0) / 25.0).max(0.0);
    wind * wind * 0.6 * (1.0 + 2.0 * abrasion) + 0.02 * abrasion * ((u - 45.0) / 20.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::super::world::testing::MemoryWorld;
    use super::super::world::{READ_ABOVE, READ_BELOW, VoxelWindow, WorldIo};
    use super::*;

    fn window_with_tree() -> VoxelWindow {
        let mut world = MemoryWorld::flat(64);
        world.tree(0, 0, 6);
        let mut w = VoxelWindow::new();
        let cols: Vec<(i32, i32)> = (-14..15).flat_map(|x| (-6..7).map(move |z| (x, z))).collect();
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 0.0);
        w
    }

    #[test]
    fn finds_the_trunk_and_its_crown() {
        let w = window_with_tree();
        let t = find_trunk(&w, 0, 0).unwrap();
        assert_eq!(t.base, [0, 65, 0]);
        assert_eq!(t.height, 6);
        assert_eq!(t.species, Species::Oak);
        assert!(count_leaves(&w, &t) > 20);
        assert!(find_trunk(&w, 1, 0).is_none());
    }

    #[test]
    fn defoliated_trees_resist_more_and_strong_winds_fell_them() {
        let w = window_with_tree();
        let t = find_trunk(&w, 0, 0).unwrap();
        assert!(critical_wind(&t, 0.1) > critical_wind(&t, 1.0) * 1.4);
        let mut rng = Rng::new(1);
        let mut s = TreeState::default();
        assert_eq!(step_tree(&mut s, &t, 40, 25.0, 0.0, 1.0, &mut rng), TreeOutcome::Standing);
        let mut felled = 0;
        for seed in 0..50 {
            let mut rng = Rng::new(seed);
            let mut s = TreeState::default();
            if step_tree(&mut s, &t, 40, 70.0, 0.0, 1.0, &mut rng) != TreeOutcome::Standing {
                felled += 1;
            }
        }
        assert_eq!(felled, 50);
    }

    #[test]
    fn felled_trunk_lies_downwind() {
        let w = window_with_tree();
        let laid = felled_trunk(&w, [0, 65, 0], 5, Species::Oak, false, Vec3::new(30.0, 0.0, 5.0));
        assert_eq!(laid.len(), 5);
        assert!(laid.iter().all(|(p, s)| p[2] == 0 && p[0] > 0 && s.contains("axis=x")));
    }

    #[test]
    fn debarking_needs_extreme_wind() {
        assert_eq!(debark_rate(55.0, 0.0), 0.0);
        assert!(debark_rate(90.0, 1.0) > debark_rate(75.0, 0.0));
        assert_eq!(stripped("minecraft:oak_log[axis=y]").as_deref(), Some("minecraft:stripped_oak_log[axis=y]"));
        assert_eq!(stripped("minecraft:stripped_oak_log[axis=y]"), None);
    }
}
