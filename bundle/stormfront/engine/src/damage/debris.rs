// Flying debris: pieces torn from the world, carried by the wind field, colliding with the
// world, breaking what they hit, breaking apart themselves, and coming to rest.
//
// Each piece is a point mass with a drag area: m dv/dt = 1/2 rho Cd A |u - v| (u - v) - m g.
// Light pieces (shingles, glass, boards) have a high area-to-mass ratio and ride the vortex's
// updraft high and far; heavy ones (logs, masonry) need extreme winds to leave the ground. On
// impact, the kinetic energy decides: the target breaks if the energy exceeds what it can
// absorb (glass first; a heavy fast piece breaks walls), and the piece fragments if its energy
// per mass exceeds its material's fragmentation energy; fragments too small to matter become
// particles only. Pieces at rest either leave a deposit (boards, broken masonry, logs) or
// vanish. The number of pieces is capped; when full, the lightest pieces are merged away.

use super::materials::{Kind, Material, classify, deposit_state};
use super::structures::DamageKind;
use super::world::VoxelWindow;
use crate::math::{Rng, Vec3};
use crate::units::G;

/// Pieces lighter than this (kg) are not simulated as debris.
pub const MIN_MASS: f64 = 1.5;
/// Pieces at least this heavy (kg) leave a block where they come to rest.
pub const DEPOSIT_MASS: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Debris {
    pub id: u32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub mass: f64,
    /// Frontal area (m^2) and drag coefficient.
    pub area: f64,
    pub cd: f64,
    /// The block state it came from (for drawing and depositing).
    pub state: String,
    pub material: Material,
    /// Edge length (m) for drawing.
    pub size: f64,
    /// Tumbling: rotation axis * rate (rad/s), and the current angle.
    pub spin: Vec3,
    pub angle: f64,
    pub age: f64,
    pub resting: f64,
    /// How many times it has broken apart.
    pub generation: u8,
    /// The tornado that lofted it, if any.
    pub tornado: Option<u32>,
    pub max_height: f64,
    pub origin: Vec3,
}

/// Something debris did, for the survey and for clients.
#[derive(Clone, Debug, PartialEq)]
pub enum DebrisEvent {
    /// Broke the block at `pos`.
    Broke { pos: [i32; 3], kind: DamageKind, energy: f64 },
    /// A log was scoured by impacts (abrasion towards debarking).
    Abraded { pos: [i32; 3] },
    /// Broke apart into `pieces` at `pos` (and some dust).
    Fragmented { pos: Vec3, pieces: u32, state: String },
    /// Came to rest, leaving `deposit` if any.
    Landed { pos: Vec3, deposit: Option<([i32; 3], String)>, distance: f64 },
}

#[derive(Debug)]
pub struct DebrisSystem {
    pub pieces: Vec<Debris>,
    pub limit: usize,
    next_id: u32,
    pub events: Vec<DebrisEvent>,
    /// Totals for the survey.
    pub total_launched: u64,
    pub total_fragmented: u64,
    pub max_distance: f64,
    rng: Rng,
}

impl DebrisSystem {
    pub fn new(limit: usize, seed: u64) -> Self {
        DebrisSystem { pieces: Vec::new(), limit, next_id: 1, events: Vec::new(), total_launched: 0, total_fragmented: 0, max_distance: 0.0, rng: Rng::new(seed ^ 0xDEB) }
    }

    /// Launch a piece made from the block `state` at `pos` (block corner) in wind `wind`.
    /// Answers false when it is too light to simulate or the system is full of heavier pieces.
    pub fn launch(&mut self, pos: [i32; 3], state: &str, wind: Vec3, tornado: Option<u32>) -> bool {
        let m = classify(state);
        if !m.debris {
            return false;
        }
        let mass = m.block_mass().max(0.5);
        if mass < MIN_MASS {
            return false;
        }
        if self.pieces.len() >= self.limit {
            // make room by dropping the lightest piece if this one is heavier
            let (i, lightest) = match self.pieces.iter().enumerate().min_by(|a, b| a.1.mass.total_cmp(&b.1.mass)) {
                Some((i, p)) => (i, p.mass),
                None => return false,
            };
            if lightest >= mass {
                return false;
            }
            self.pieces.swap_remove(i);
        }
        let size = m.fill.cbrt();
        let center = Vec3::new(pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5);
        let r = &mut self.rng;
        let vel = wind * r.range(0.15, 0.4) + Vec3::new(r.normal(), r.range(0.5, 2.0), r.normal()) * 1.5;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pieces.push(Debris {
            id,
            pos: center,
            vel,
            mass,
            // a plate-like piece presents more area than a cube of the same volume
            area: (size * size * if m.fill < 0.6 { 1.6 } else { 1.0 }).max(0.05),
            cd: m.cd,
            state: state.to_string(),
            material: m,
            size,
            spin: r.unit_vector() * r.range(2.0, 12.0),
            angle: 0.0,
            age: 0.0,
            resting: 0.0,
            generation: 0,
            tornado,
            max_height: center.y,
            origin: center,
        });
        self.total_launched += 1;
        true
    }

    /// Advance all pieces by `dt` in the wind field `wind_at`, colliding with `window`.
    /// Pieces outside the cached world fall to `ground_y` and vanish there.
    pub fn step(&mut self, dt: f64, window: &mut VoxelWindow, rho: f64, ground_y: f64, wind_at: &dyn Fn(Vec3) -> Vec3) {
        let substeps = 2;
        let h = dt / substeps as f64;
        let mut born: Vec<Debris> = Vec::new();
        let mut i = 0;
        while i < self.pieces.len() {
            let mut remove = false;
            for _ in 0..substeps {
                let p = &mut self.pieces[i];
                let u = wind_at(p.pos);
                let rel = u - p.vel;
                let speed = rel.length();
                // drag acceleration, then gravity (semi-implicit for stability of light pieces)
                let k = 0.5 * rho * p.cd * p.area / p.mass;
                let relax = (k * speed * h).min(0.95);
                p.vel += rel * relax;
                p.vel.y -= G * h;
                p.vel = p.vel.clamp_length(160.0);
                let next = p.pos + p.vel * h;
                p.angle += p.spin.length() * h;
                p.age += h;
                let cell = next.block();
                match window.material(cell).copied() {
                    None => {
                        // outside what we hold: free flight over flat ground
                        if next.y <= ground_y {
                            remove = true;
                            let d = (next - p.origin).horizontal_length();
                            self.max_distance = self.max_distance.max(d);
                            self.events.push(DebrisEvent::Landed { pos: next, deposit: None, distance: d });
                            break;
                        }
                        p.pos = next;
                    }
                    Some(target) if target.is_obstacle() => {
                        let impact = p.vel.length();
                        let energy = 0.5 * p.mass * impact * impact;
                        // the target
                        let tq = super::structures::block_quality(cell);
                        if target.kind != Kind::Unbreakable && !target.terrain && energy > target.impact_energy * tq {
                            let kind = match target.kind {
                                Kind::Glass | Kind::GlassPane => DamageKind::Glass,
                                Kind::Leaves => DamageKind::Defoliation,
                                Kind::Log | Kind::StrippedLog => DamageKind::DebrisImpact,
                                _ => DamageKind::DebrisImpact,
                            };
                            if matches!(target.kind, Kind::Log | Kind::StrippedLog) {
                                self.events.push(DebrisEvent::Abraded { pos: cell });
                            } else {
                                window.remove(cell);
                                self.events.push(DebrisEvent::Broke { pos: cell, kind, energy });
                                // it punches through, losing what the target absorbed
                                let keep = (1.0 - target.impact_energy * tq / energy).clamp(0.0, 1.0).sqrt();
                                p.vel *= keep;
                                p.pos = next;
                                continue;
                            }
                        } else if matches!(target.kind, Kind::Log | Kind::StrippedLog) && energy > 800.0 {
                            self.events.push(DebrisEvent::Abraded { pos: cell });
                        }
                        // the piece itself
                        let specific = energy / p.mass;
                        if specific > p.material.fragment_energy && p.generation < 3 {
                            let n = ((specific / p.material.fragment_energy).sqrt().floor() as u32).clamp(2, 5);
                            self.total_fragmented += 1;
                            self.events.push(DebrisEvent::Fragmented { pos: p.pos, pieces: n, state: p.state.clone() });
                            let piece_mass = p.mass / n as f64;
                            if piece_mass >= MIN_MASS {
                                for k in 0..n {
                                    let mut c = p.clone();
                                    c.mass = piece_mass * self.rng.range(0.6, 1.4);
                                    c.size = p.size * (c.mass / p.mass).cbrt();
                                    c.area = (c.size * c.size).max(0.02);
                                    c.generation += 1;
                                    c.vel = reflect(p.vel, p.pos, next) * self.rng.range(0.2, 0.5) + self.rng.unit_vector() * (impact * 0.15);
                                    c.spin = self.rng.unit_vector() * self.rng.range(5.0, 20.0);
                                    c.id = self.next_id.wrapping_add(k);
                                    born.push(c);
                                }
                                self.next_id = self.next_id.wrapping_add(n);
                            }
                            remove = true;
                            break;
                        }
                        // bounce off the face it crossed, losing most of its speed
                        p.vel = reflect(p.vel, p.pos, next) * 0.3;
                        p.spin *= 0.7;
                    }
                    Some(_) => {
                        p.pos = next;
                    }
                }
                p.max_height = p.max_height.max(p.pos.y);
                // resting: slow, on something solid, and the wind can't lift it
                let below = window.material((p.pos - Vec3::new(0.0, 0.6, 0.0)).block()).copied();
                let grounded = below.is_some_and(|m| m.is_solid());
                if grounded && p.vel.length() < 1.5 && rel_drag(p, wind_at(p.pos), rho) < 0.6 * p.mass * G {
                    p.resting += h;
                    p.vel *= 0.5;
                } else {
                    p.resting = 0.0;
                }
                if p.resting > 1.5 || p.age > 240.0 {
                    remove = true;
                    let d = (p.pos - p.origin).horizontal_length();
                    self.max_distance = self.max_distance.max(d);
                    let deposit = if p.mass >= DEPOSIT_MASS && p.generation == 0 || p.material.kind == Kind::Log && p.mass > 150.0 {
                        let cell = p.pos.block();
                        let axis = if p.vel.x.abs() > p.vel.z.abs() { 'x' } else { 'z' };
                        match (window.material(cell), deposit_state(&p.state, axis)) {
                            (Some(m), Some(s)) if m.is_air() || m.kind == Kind::Plant => {
                                window.set_state(cell, &s);
                                Some((cell, s))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    self.events.push(DebrisEvent::Landed { pos: p.pos, deposit, distance: d });
                    break;
                }
            }
            if remove {
                self.pieces.swap_remove(i);
            } else {
                i += 1;
            }
        }
        for b in born {
            if self.pieces.len() < self.limit {
                self.pieces.push(b);
            }
        }
    }

    /// Lofted debris mass (kg) within `radius` of `center` and above `min_height` of ground.
    pub fn lofted_mass(&self, center: Vec3, radius: f64, ground: f64) -> f64 {
        self.pieces
            .iter()
            .filter(|p| (p.pos - center).horizontal_length() < radius && p.pos.y > ground + 3.0)
            .map(|p| p.mass)
            .sum()
    }

    pub fn clear(&mut self) {
        self.pieces.clear();
    }
}

fn rel_drag(p: &Debris, u: Vec3, rho: f64) -> f64 {
    let s = (u - p.vel).length();
    0.5 * rho * p.cd * p.area * s * s
}

/// Reflect `v` off the face of the cell of `to` crossed coming from `from`.
fn reflect(v: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let a = from.block();
    let b = to.block();
    let mut r = v;
    if a[1] != b[1] {
        r.y = -r.y;
    } else if a[0] != b[0] {
        r.x = -r.x;
    } else if a[2] != b[2] {
        r.z = -r.z;
    } else {
        r = -r;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::super::world::testing::MemoryWorld;
    use super::super::world::{READ_ABOVE, READ_BELOW, WorldIo};
    use super::*;

    fn window(world: &mut MemoryWorld, r: i32) -> VoxelWindow {
        let mut w = VoxelWindow::new();
        let cols: Vec<(i32, i32)> = (-r..=r).flat_map(|x| (-r..=r).map(move |z| (x, z))).collect();
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 0.0);
        w
    }

    #[test]
    fn light_debris_travels_farther_than_heavy() {
        let mut world = MemoryWorld::flat(64);
        let mut w = window(&mut world, 4);
        let mut sys = DebrisSystem::new(100, 1);
        // a strong wind with an updraft near the source, weaker downstream
        let wind = |p: Vec3| Vec3::new(40.0, if p.x < 30.0 { 25.0 } else { -2.0 }, 0.0);
        assert!(sys.launch([0, 66, 0], "minecraft:oak_slab[type=bottom]", Vec3::new(40.0, 0.0, 0.0), None));
        assert!(sys.launch([0, 66, 1], "minecraft:stone_bricks", Vec3::new(40.0, 0.0, 0.0), None));
        let (light, heavy) = (sys.pieces[0].id, sys.pieces[1].id);
        let mut far = std::collections::HashMap::new();
        for _ in 0..2000 {
            sys.step(0.05, &mut w, 1.2, 64.0, &wind);
            for p in &sys.pieces {
                far.insert(p.id, (p.pos - p.origin).horizontal_length());
            }
        }
        assert!(far[&light] > far[&heavy] * 2.0, "{far:?}");
    }

    #[test]
    fn fast_heavy_debris_breaks_glass_and_fragments_on_stone() {
        let mut world = MemoryWorld::flat(64);
        world.set([3, 66, 0], "minecraft:glass_pane");
        world.fill([6, 65, -1], [6, 68, 1], "minecraft:stone");
        let mut w = window(&mut world, 8);
        let mut sys = DebrisSystem::new(100, 2);
        sys.launch([0, 66, 0], "minecraft:bricks", Vec3::ZERO, None);
        sys.pieces[0].vel = Vec3::new(70.0, 0.0, 0.0);
        sys.pieces[0].pos = Vec3::new(0.5, 66.5, 0.5);
        let calm = |_: Vec3| Vec3::new(70.0, 0.0, 0.0);
        for _ in 0..10 {
            sys.step(0.02, &mut w, 1.2, 64.0, &calm);
        }
        assert_eq!(w.state([3, 66, 0]), Some("minecraft:air"), "glass broken");
        assert!(sys.events.iter().any(|e| matches!(e, DebrisEvent::Broke { kind: DamageKind::Glass, .. })));
        assert!(sys.events.iter().any(|e| matches!(e, DebrisEvent::Fragmented { .. })), "{:?}", sys.events);
    }

    #[test]
    fn calm_pieces_come_to_rest_and_deposit() {
        let mut world = MemoryWorld::flat(64);
        let mut w = window(&mut world, 4);
        let mut sys = DebrisSystem::new(100, 3);
        sys.launch([0, 67, 0], "minecraft:oak_log[axis=y]", Vec3::ZERO, None);
        sys.pieces[0].vel = Vec3::ZERO;
        let calm = |_: Vec3| Vec3::ZERO;
        for _ in 0..200 {
            sys.step(0.05, &mut w, 1.2, 64.0, &calm);
        }
        assert!(sys.pieces.is_empty());
        let landed = sys.events.iter().find_map(|e| match e {
            DebrisEvent::Landed { deposit, .. } => Some(deposit.clone()),
            _ => None,
        });
        let (cell, s) = landed.unwrap().unwrap();
        assert_eq!(cell[1], 65);
        assert!(s.contains("oak_log"));
    }

    #[test]
    fn the_cap_keeps_the_heaviest() {
        let mut sys = DebrisSystem::new(2, 4);
        assert!(sys.launch([0, 70, 0], "minecraft:oak_slab", Vec3::ZERO, None));
        assert!(sys.launch([1, 70, 0], "minecraft:oak_slab", Vec3::ZERO, None));
        assert!(sys.launch([2, 70, 0], "minecraft:stone_bricks", Vec3::ZERO, None));
        assert_eq!(sys.pieces.len(), 2);
        assert!(sys.pieces.iter().any(|p| p.state == "minecraft:stone_bricks"));
        assert!(!sys.launch([3, 70, 0], "minecraft:glass", Vec3::ZERO, None), "glass shatters instead");
    }
}
