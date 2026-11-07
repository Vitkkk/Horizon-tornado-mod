// Wind loads on blocks and their resistance, for buildings and anything else built.
//
// Load: q = 1/2 rho V^2 times a pressure coefficient summed over the block's exposed faces
// (windward pressure, leeward and side suction, roof uplift, partial internal pressure).
// Resistance: the material's strength raised by what holds the block (the block below, its
// neighbours in the same wall, the load above, anchoring to the foundation), scaled by a
// per-block and a per-building construction quality, and lowered by fatigue under sustained
// loading. A block fails when the load exceeds its resistance; losing it changes the exposure
// and support of its neighbours, which is how damage progresses from glass and roof covering
// to walls and whole buildings, and why it can cascade.

use super::materials::{Kind, Material};
use super::world::VoxelWindow;
use crate::math::{Vec3, hash01};

/// The six neighbour offsets: +x, -x, +y, -y, +z, -z.
pub const FACES: [[i32; 3]; 6] = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

/// What kind of damage a failure is, as a surveyor would record it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DamageKind {
    /// Flowers, grass and crops stripped.
    Vegetation,
    Defoliation,
    TreeUprooted,
    TreeSnapped,
    Debarked,
    Glass,
    /// Roof covering (stairs, slabs on top).
    RoofCover,
    RoofStructure,
    Wall,
    /// A part left without support fell.
    Collapse,
    Contents,
    /// Floor or foundation blocks swept from the ground.
    SweptFoundation,
    /// Masonry, stone, concrete or metal failed.
    Engineered,
    ScourLight,
    ScourModerate,
    ScourExtreme,
    DebrisImpact,
}

impl DamageKind {
    pub const ALL: [DamageKind; 17] = [
        DamageKind::Vegetation,
        DamageKind::Defoliation,
        DamageKind::TreeUprooted,
        DamageKind::TreeSnapped,
        DamageKind::Debarked,
        DamageKind::Glass,
        DamageKind::RoofCover,
        DamageKind::RoofStructure,
        DamageKind::Wall,
        DamageKind::Collapse,
        DamageKind::Contents,
        DamageKind::SweptFoundation,
        DamageKind::Engineered,
        DamageKind::ScourLight,
        DamageKind::ScourModerate,
        DamageKind::ScourExtreme,
        DamageKind::DebrisImpact,
    ];
    pub fn name(self) -> &'static str {
        match self {
            DamageKind::Vegetation => "vegetação rasteira",
            DamageKind::Defoliation => "desfolhamento",
            DamageKind::TreeUprooted => "árvores arrancadas",
            DamageKind::TreeSnapped => "troncos quebrados",
            DamageKind::Debarked => "descascamento",
            DamageKind::Glass => "vidros",
            DamageKind::RoofCover => "cobertura do telhado",
            DamageKind::RoofStructure => "estrutura do telhado",
            DamageKind::Wall => "paredes",
            DamageKind::Collapse => "colapso",
            DamageKind::Contents => "conteúdo/mobília",
            DamageKind::SweptFoundation => "fundação varrida",
            DamageKind::Engineered => "alvenaria/concreto",
            DamageKind::ScourLight => "scouring leve",
            DamageKind::ScourModerate => "scouring moderado",
            DamageKind::ScourExtreme => "scouring extremo",
            DamageKind::DebrisImpact => "impacto de detritos",
        }
    }
    pub fn index(self) -> usize {
        DamageKind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
}

/// A block's wind load and resistance right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadCheck {
    /// Wind pressure on the block (Pa).
    pub load: f64,
    /// What it withstands (Pa), fatigue not included.
    pub resistance: f64,
    /// Exposed faces.
    pub exposed: u8,
    pub top_exposed: bool,
    /// Whether it rests on terrain (a floor or foundation).
    pub on_terrain: bool,
    pub supported_below: bool,
}

/// Whether a neighbour lets wind reach the shared face.
#[inline]
fn open(m: &Material) -> bool {
    matches!(m.kind, Kind::Air | Kind::Plant | Kind::Attachment | Kind::Fluid) || m.fill < 0.3
}

/// Whether the open cell `q` is enclosed air (a room): something solid above within four
/// blocks and on both sides along x or z. Cheap and conservative.
pub fn sheltered(window: &VoxelWindow, q: [i32; 3]) -> bool {
    let solid = |p: [i32; 3]| window.material(p).is_some_and(|m| m.is_solid());
    let roofed = (1..=4).any(|k| solid([q[0], q[1] + k, q[2]]));
    if !roofed {
        return false;
    }
    let both = |dx: i32, dz: i32| (1..=4).any(|k| solid([q[0] + dx * k, q[1], q[2] + dz * k])) && (1..=4).any(|k| solid([q[0] - dx * k, q[1], q[2] - dz * k]));
    both(1, 0) || both(0, 1)
}

/// Per-building construction quality, 0.7..1.35: some buildings are well anchored and built,
/// others are not, so similar winds do different damage (a surveyor's lower/upper bound).
pub fn building_quality(x: i32, z: i32) -> f64 {
    let h = hash01(x.div_euclid(12), 0, z.div_euclid(12), 0xB01D);
    0.7 + 0.65 * h
}

/// Per-block quality, 0.8..1.25 (nails, mortar joints, defects).
pub fn block_quality(p: [i32; 3]) -> f64 {
    0.8 + 0.45 * hash01(p[0], p[1], p[2], 0x51AB)
}

/// The load and resistance of the block at `p` (material `m`) under wind `wind`.
pub fn check(window: &VoxelWindow, p: [i32; 3], m: &Material, wind: Vec3, rho: f64) -> LoadCheck {
    let speed = wind.length();
    let dir = if speed > 1e-6 { wind / speed } else { Vec3::ZERO };
    let horiz = dir.horizontal();
    let h_mag = horiz.length();
    let mut c = 0.0;
    let mut exposed = 0u8;
    let mut top_exposed = false;
    let mut support: f64 = 0.0;
    let mut supported_below = false;
    let mut on_terrain = false;
    let mut sides_solid = 0;
    for (i, f) in FACES.iter().enumerate() {
        let q = [p[0] + f[0], p[1] + f[1], p[2] + f[2]];
        let nm = window.material(q).copied();
        let n_open = nm.as_ref().is_none_or(open);
        let n = Vec3::new(f[0] as f64, f[1] as f64, f[2] as f64);
        if n_open {
            exposed += 1;
            if sheltered(window, q) {
                // a face against a building's enclosed air sees only internal pressure
                c += 0.25;
                continue;
            }
            match i {
                2 => {
                    // roof uplift: suction over a flat top in a horizontal wind, plus the updraft
                    top_exposed = true;
                    c += 0.9 * h_mag + 0.6 * dir.y.max(0.0);
                }
                3 => {
                    // an exposed underside (eaves, overhangs) is pushed up
                    c += 0.7 * h_mag + 0.8 * dir.y.max(0.0);
                }
                _ => {
                    let d = n.dot(horiz);
                    c += 0.8 * (-d).max(0.0) + 0.5 * d.max(0.0) + 0.35 * (1.0 - d.abs());
                }
            }
        } else if let Some(nm) = nm {
            match i {
                3 => {
                    supported_below = nm.is_solid();
                    on_terrain = nm.terrain;
                    support += if nm.is_solid() { 1.0 } else { 0.0 };
                }
                2 => support += if nm.is_solid() { 0.25 } else { 0.0 },
                _ => {
                    if nm.is_solid() {
                        sides_solid += 1;
                        support += 0.35;
                    }
                }
            }
        }
    }
    let c = c.min(1.6);
    let load = 0.5 * rho * speed * speed * c;
    // roof-like blocks (top exposed) are held by fastenings, not by the bulk of the wall
    let support_term = if top_exposed {
        0.5 * (support.min(1.0)) + 0.1 * sides_solid as f64
    } else {
        support
    };
    let mut resistance = m.strength * (1.0 + m.connect * support_term);
    if on_terrain && m.is_structural() {
        // anchored to the foundation
        resistance *= 2.5;
    }
    if !supported_below && sides_solid < 2 && m.is_structural() {
        // a cantilever or a hanging remnant
        resistance *= 0.4;
    }
    resistance *= block_quality(p) * if m.is_structural() { building_quality(p[0], p[2]) } else { 1.0 };
    LoadCheck { load, resistance, exposed, top_exposed, on_terrain, supported_below }
}

/// How a surveyor would describe the failure of a structural block.
pub fn classify_failure(m: &Material, state: &str, lc: &LoadCheck, height_above_ground: i32) -> DamageKind {
    let name = state.split('[').next().unwrap_or(state);
    match m.kind {
        Kind::Glass | Kind::GlassPane => DamageKind::Glass,
        Kind::Leaves => DamageKind::Defoliation,
        Kind::Plant => DamageKind::Vegetation,
        Kind::Furniture | Kind::Attachment => DamageKind::Contents,
        Kind::Masonry | Kind::Stone | Kind::Concrete | Kind::Metal => {
            if lc.on_terrain { DamageKind::SweptFoundation } else { DamageKind::Engineered }
        }
        _ => {
            if lc.on_terrain && height_above_ground <= 1 {
                DamageKind::SweptFoundation
            } else if lc.top_exposed && (name.ends_with("_stairs") || name.ends_with("_slab")) {
                DamageKind::RoofCover
            } else if lc.top_exposed && height_above_ground > 2 {
                DamageKind::RoofStructure
            } else {
                DamageKind::Wall
            }
        }
    }
}

/// Structural blocks around `removed` left with no path down to the terrain: they fall.
/// The search is bounded (`limit` blocks); a component too large to finish is assumed held.
pub fn unsupported_after(window: &VoxelWindow, removed: &[[i32; 3]], limit: usize) -> Vec<[i32; 3]> {
    use std::collections::{HashSet, VecDeque};
    let mut falling = Vec::new();
    let mut settled: HashSet<[i32; 3]> = HashSet::new();
    for r in removed {
        for f in FACES {
            let start = [r[0] + f[0], r[1] + f[1], r[2] + f[2]];
            if settled.contains(&start) {
                continue;
            }
            let Some(m) = window.material(start) else { continue };
            if !m.is_structural() || !m.is_solid() {
                continue;
            }
            let mut seen: HashSet<[i32; 3]> = HashSet::new();
            let mut queue = VecDeque::new();
            seen.insert(start);
            queue.push_back(start);
            let mut grounded = false;
            while let Some(p) = queue.pop_front() {
                if seen.len() > limit {
                    grounded = true;
                    break;
                }
                for f in FACES {
                    let q = [p[0] + f[0], p[1] + f[1], p[2] + f[2]];
                    let Some(qm) = window.material(q) else {
                        // the edge of what we hold: assume it continues to the ground
                        grounded = true;
                        continue;
                    };
                    if qm.terrain || qm.kind == Kind::Unbreakable || (qm.kind == Kind::Log && f[1] == -1) {
                        if f[1] == -1 || f == [0, 0, 0] {
                            grounded = true;
                        } else if qm.terrain {
                            // resting sideways against terrain (a hillside) holds too
                            grounded = true;
                        }
                        continue;
                    }
                    if qm.is_structural() && qm.is_solid() && seen.insert(q) {
                        queue.push_back(q);
                    }
                }
                if grounded {
                    break;
                }
            }
            if grounded {
                settled.extend(seen);
            } else {
                falling.extend(seen.iter().copied());
                settled.extend(seen);
            }
        }
    }
    falling.sort_unstable();
    falling.dedup();
    falling
}

#[cfg(test)]
mod tests {
    use super::super::world::testing::MemoryWorld;
    use super::super::world::{READ_ABOVE, READ_BELOW, VoxelWindow, WorldIo};
    use super::*;

    fn window_with_house() -> (VoxelWindow, MemoryWorld) {
        let mut world = MemoryWorld::flat(64);
        world.house(0, 0);
        let mut w = VoxelWindow::new();
        let cols: Vec<(i32, i32)> = (-2..10).flat_map(|x| (-2..9).map(move |z| (x, z))).collect();
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 0.0);
        (w, world)
    }

    fn fail_speed(w: &VoxelWindow, p: [i32; 3], dir: Vec3) -> f64 {
        let m = *w.material(p).unwrap();
        let lc = check(w, p, &m, dir * 50.0, 1.2);
        // load scales with V^2
        50.0 * (lc.resistance / lc.load).sqrt()
    }

    #[test]
    fn glass_fails_before_roof_before_walls_before_floor() {
        let (w, _) = window_with_house();
        let east = Vec3::new(1.0, 0.0, 0.0);
        let glass = fail_speed(&w, [0, 67, 1], east); // a window on the west wall facing the wind
        let roof = fail_speed(&w, [3, 70, 3], east);
        let wall = fail_speed(&w, [0, 66, 3], east);
        let floor = fail_speed(&w, [3, 65, 3], east);
        assert!(glass < roof, "glass {glass} roof {roof}");
        assert!(roof < wall, "roof {roof} wall {wall}");
        assert!(wall < floor, "wall {wall} floor {floor}");
        assert!((25.0..48.0).contains(&glass), "{glass}");
        assert!((35.0..62.0).contains(&roof), "{roof}");
        assert!((45.0..85.0).contains(&wall), "{wall}");
    }

    #[test]
    fn a_wall_without_its_roof_is_weaker() {
        let (mut w, _) = window_with_house();
        let east = Vec3::new(1.0, 0.0, 0.0);
        let before = fail_speed(&w, [0, 69, 3], east);
        w.remove([0, 70, 3]);
        let after = fail_speed(&w, [0, 69, 3], east);
        assert!(after < before, "{after} {before}");
    }

    #[test]
    fn a_floating_remnant_is_found_unsupported() {
        let (mut w, _) = window_with_house();
        // cut a column of the west wall free: remove everything around [0, 69, 3] but it
        for y in 65..=70 {
            for (x, z) in [(0, 2), (0, 4), (1, 3)] {
                w.remove([x, y, z]);
            }
        }
        for y in 65..=68 {
            w.remove([0, y, 3]);
        }
        w.remove([0, 70, 3]);
        let falling = unsupported_after(&w, &[[0, 68, 3]], 5000);
        assert!(falling.contains(&[0, 69, 3]), "{falling:?}");
        // the rest of the house still stands on its floor
        let none = unsupported_after(&w, &[[6, 66, 3]], 5000);
        assert!(none.is_empty(), "{none:?}");
    }
}
