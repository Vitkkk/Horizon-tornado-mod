// What makes particles, each step.

use super::{Category, Particle, ParticleSystem, block_color};
use crate::math::{Vec3, smoothstep};
use crate::render_bridge::Shape;
use crate::storms::Storm;
use crate::storms::system::StormSystem;
use crate::tornado::{Morphology, Tornado};

fn key(kind: u64, id: u32, sub: u32) -> u64 {
    kind << 48 | (id as u64) << 16 | sub as u64
}

/// Everything for one client step.
pub fn emit_all(ps: &mut ParticleSystem, sys: &StormSystem, camera: Vec3, dt: f64) {
    for t in &sys.tornadoes {
        let d = (t.pos - camera).horizontal_length();
        if d > 3000.0 {
            continue;
        }
        funnel(ps, t, camera, dt);
        ground_cloud(ps, t, dt);
        subvortex_tubes(ps, t, dt);
    }
    for s in &sys.storms {
        let d = (s.pos - camera).horizontal_length();
        if d > s.extent() + 2500.0 {
            continue;
        }
        cloud_base(ps, s, sys.ground_y, camera, dt);
        rain_shafts(ps, s, sys, camera, dt);
    }
    local_precipitation(ps, sys, camera, dt);
}

/// The condensation funnel: droplets seeded inside the funnel at every level, which then ride
/// the vortex's swirl and updraft (and evaporate when they leave the funnel).
fn funnel(ps: &mut ParticleSystem, t: &Tornado, camera: Vec3, dt: f64) {
    let levels = &t.funnel.levels;
    if levels.is_empty() {
        return;
    }
    let dark = match t.funnel.morphology {
        Morphology::RainWrapped | Morphology::Wedge => 0.62,
        _ => 0.78,
    };
    let near = 1.0 - smoothstep(600.0, 2500.0, (t.pos - camera).horizontal_length());
    for (k, l) in levels.iter().enumerate() {
        if l.radius <= 0.3 {
            continue;
        }
        // the rate scales with the funnel's surface at this level
        let rate = (8.0 + 2.2 * l.radius).min(260.0) * (0.35 + 0.65 * near);
        let n = ps.quota(key(1, t.id, k as u32), rate, dt);
        for _ in 0..n {
            let ang = ps.rng.range(0.0, std::f64::consts::TAU);
            // mostly near the visible surface, some inside for depth
            let rr = l.radius * (0.55 + 0.45 * ps.rng.f64().sqrt());
            let h = l.height + ps.rng.range(-0.5, 0.5) * (t.cloud_base / levels.len() as f64);
            let pos = t.pos + l.offset + Vec3::new(ang.cos() * rr, h, -ang.sin() * rr);
            let g = (dark * 255.0 * ps.rng.range(0.85, 1.05)) as u8;
            let size = (l.radius * 0.35).clamp(1.2, 16.0) as f32 * ps.rng.range(0.7, 1.3) as f32;
            let p = Particle {
                pos,
                vel: t.wind_at(pos),
                age: 0.0,
                life: ps.rng.range(1.2, 2.6) as f32,
                size,
                grow: size * 0.25,
                color: [g, g, (g as f32 * 1.03).min(255.0) as u8, (150.0 + 60.0 * (1.0 - t.rain_wrap)) as u8],
                shape: Shape::Puff,
                tau: 0.06,
                fall: 0.0,
                tornado: t.id,
                condensed: true,
                seed: ps.rng.below(256) as u8,
            };
            if !ps.spawn(Category::Funnel, p) {
                return;
            }
        }
    }
}

/// Dust, dirt and small debris lifted where the near-surface wind is damaging; it is flung
/// outward and lofted by the vortex's own flow. Heavier with the debris the server reports.
fn ground_cloud(ps: &mut ParticleSystem, t: &Tornado, dt: f64) {
    let r = t.radius_of_wind(30.0);
    if r <= 1.0 {
        return;
    }
    let load = (t.debris_load / 2000.0).min(3.0);
    let rate = (20.0 + 0.9 * r + 60.0 * load).min(500.0);
    let n = ps.quota(key(2, t.id, 0), rate, dt);
    for _ in 0..n {
        let ang = ps.rng.range(0.0, std::f64::consts::TAU);
        let rr = r * ps.rng.f64().sqrt();
        let pos = t.pos + Vec3::new(ang.cos() * rr, ps.rng.range(0.2, 3.0), -ang.sin() * rr);
        let debris = ps.rng.chance(0.15 + 0.2 * load / 3.0);
        let (color, shape, tau, fall, size) = if debris {
            let c = if ps.rng.chance(0.5) { [95, 75, 55, 255] } else { [70, 64, 58, 255] };
            (c, Shape::Chunk, 0.8, 3.0, ps.rng.range(0.15, 0.6) as f32)
        } else {
            ([120, 100, 78, 120], Shape::Puff, 0.35, 0.4, ps.rng.range(1.5, 5.0) as f32)
        };
        let p = Particle {
            pos,
            vel: t.wind_at(pos) * 0.5,
            age: 0.0,
            life: ps.rng.range(2.0, 5.0) as f32,
            size,
            grow: if debris { 0.0 } else { size * 0.5 },
            color,
            shape,
            tau,
            fall,
            tornado: t.id,
            condensed: false,
            seed: ps.rng.below(256) as u8,
        };
        if !ps.spawn(Category::Ground, p) {
            return;
        }
    }
}

/// The thin condensation tubes of suction vortices, from the ground to part of the way up.
fn subvortex_tubes(ps: &mut ParticleSystem, t: &Tornado, dt: f64) {
    for (i, v) in t.multivortex.vortices.iter().enumerate() {
        let e = v.envelope();
        if e < 0.3 {
            continue;
        }
        let n = ps.quota(key(3, t.id, i as u32), 70.0 * e, dt);
        let top = t.cloud_base * 0.45;
        for _ in 0..n {
            let h = ps.rng.f64().powf(0.7) * top;
            let ang = ps.rng.range(0.0, std::f64::consts::TAU);
            let rr = v.core * 0.6 * ps.rng.f64().sqrt();
            let pos = t.pos + v.offset() + Vec3::new(ang.cos() * rr, h, -ang.sin() * rr);
            let p = Particle {
                pos,
                vel: t.wind_at(pos),
                age: 0.0,
                life: ps.rng.range(0.6, 1.2) as f32,
                size: (v.core * 0.5).clamp(1.0, 6.0) as f32,
                grow: 0.5,
                color: [175, 172, 168, 170],
                shape: Shape::Puff,
                tau: 0.05,
                fall: 0.0,
                tornado: t.id,
                condensed: false,
                seed: ps.rng.below(256) as u8,
            };
            if !ps.spawn(Category::Funnel, p) {
                return;
            }
        }
    }
}

/// A storm's base: a ceiling of dark cloud over the updraft, lowered and rotating over the
/// mesocyclone (the wall cloud), with the rear-flank downdraft's clear slot cut out of it.
fn cloud_base(ps: &mut ParticleSystem, s: &Storm, ground_y: f64, camera: Vec3, dt: f64) {
    if s.updraft < 3.0 {
        return;
    }
    let base_y = ground_y + s.cloud_base;
    let radius = (s.updraft_radius * 1.6).min(900.0);
    let rate = 25.0 * (s.updraft / 30.0).min(1.5);
    let n = ps.quota(key(4, s.id, 0), rate, dt);
    let meso = s.meso_center();
    let fwd = s.motion.normalized();
    for _ in 0..n {
        let ang = ps.rng.range(0.0, std::f64::consts::TAU);
        let rr = radius * ps.rng.f64().sqrt();
        let mut pos = s.pos + Vec3::new(ang.cos() * rr, 0.0, -ang.sin() * rr);
        // only near the viewer: far bases are out of view distance anyway
        if (pos - camera).horizontal_length() > 1100.0 {
            continue;
        }
        let dm = (pos - meso).horizontal();
        let near_meso = 1.0 - smoothstep(s.meso.radius * 0.3, s.meso.radius * 0.8, dm.length());
        // RFD clear slot: a dry arc behind the mesocyclone
        if s.rfd > 0.35 && dm.length() > s.meso.radius * 0.35 && dm.length() < s.meso.radius * 1.3 && dm.normalized().dot(-fwd) > 0.55 {
            continue;
        }
        let lowered = near_meso * s.meso.strength * 0.3 * s.cloud_base;
        pos.y = base_y - lowered + ps.rng.range(-4.0, 10.0);
        let rot = if dm.length() > 1.0 { dm.normalized().cyclonic_tangent() * (s.meso.v_rot * 0.5 * near_meso) } else { Vec3::ZERO };
        let g = (90.0 - 30.0 * near_meso + ps.rng.range(-10.0, 10.0)) as u8;
        let size = ps.rng.range(18.0, 42.0) as f32 * (1.0 - 0.4 * near_meso as f32);
        let p = Particle {
            pos,
            vel: s.motion + rot,
            age: 0.0,
            life: ps.rng.range(14.0, 26.0) as f32,
            size,
            grow: 0.4,
            color: [g, g, (g as f32 * 1.08).min(255.0) as u8, 190],
            shape: Shape::Puff,
            tau: 10.0,
            fall: 0.0,
            tornado: 0,
            condensed: false,
            seed: ps.rng.below(256) as u8,
        };
        if !ps.spawn(Category::Cloud, p) {
            return;
        }
    }
}

/// Grey shafts of heavy rain hanging from distant cores.
fn rain_shafts(ps: &mut ParticleSystem, s: &Storm, sys: &StormSystem, camera: Vec3, dt: f64) {
    if s.precip < 0.1 {
        return;
    }
    let n = ps.quota(key(5, s.id, 0), 6.0 * s.precip, dt);
    for _ in 0..n {
        let ang = ps.rng.range(0.0, std::f64::consts::TAU);
        let rr = s.updraft_radius * 3.0 * ps.rng.f64().sqrt();
        let at = s.pos + Vec3::new(ang.cos() * rr, 0.0, -ang.sin() * rr);
        let d = (at - camera).horizontal_length();
        if !(120.0..1500.0).contains(&d) {
            continue;
        }
        let q = s.hydrometeors(at, 500.0).rain;
        if q < 0.6 {
            continue;
        }
        let w = ps.rng.range(25.0, 60.0) as f32;
        let pos = Vec3::new(at.x, sys.ground_y + 2.0 * w as f64, at.z);
        let a = (40.0 + 25.0 * q).min(140.0) as u8;
        let p = Particle {
            pos,
            vel: s.motion,
            age: 0.0,
            life: ps.rng.range(6.0, 10.0) as f32,
            size: w,
            grow: 0.0,
            color: [105, 110, 118, a],
            shape: Shape::Sheet,
            tau: 10.0,
            fall: 0.0,
            tornado: 0,
            condensed: false,
            seed: ps.rng.below(256) as u8,
        };
        if !ps.spawn(Category::Precip, p) {
            return;
        }
    }
}

/// Rain and hail around the viewer, as heavy as the storm's contents above them.
fn local_precipitation(ps: &mut ParticleSystem, sys: &StormSystem, camera: Vec3, dt: f64) {
    let mut rain = 0.0;
    let mut hail = 0.0;
    for s in &sys.storms {
        let q = s.hydrometeors(camera, 400.0);
        rain += q.rain;
        hail += q.hail;
    }
    if rain < 0.05 && hail < 0.05 {
        return;
    }
    let wind = sys.env.wind(10.0);
    let n = ps.quota(key(6, 0, 0), 220.0 * rain.min(4.0), dt);
    for _ in 0..n {
        let pos = camera + Vec3::new(ps.rng.range(-28.0, 28.0), ps.rng.range(6.0, 22.0), ps.rng.range(-28.0, 28.0));
        let p = Particle {
            pos,
            vel: Vec3::new(wind.x, -8.0, wind.z),
            age: 0.0,
            life: 2.5,
            size: 0.05,
            grow: 0.0,
            color: [170, 180, 200, 130],
            shape: Shape::Streak,
            tau: 0.3,
            fall: 8.0,
            tornado: 0,
            condensed: false,
            seed: 0,
        };
        if !ps.spawn(Category::Precip, p) {
            break;
        }
    }
    let n = ps.quota(key(7, 0, 0), 60.0 * hail.min(3.0), dt);
    for _ in 0..n {
        let pos = camera + Vec3::new(ps.rng.range(-20.0, 20.0), ps.rng.range(8.0, 24.0), ps.rng.range(-20.0, 20.0));
        let p = Particle {
            pos,
            vel: Vec3::new(wind.x, -18.0, wind.z),
            age: 0.0,
            life: 2.0,
            size: ps.rng.range(0.05, 0.2) as f32,
            grow: 0.0,
            color: [235, 240, 245, 230],
            shape: Shape::Pellet,
            tau: 0.5,
            fall: 18.0,
            tornado: 0,
            condensed: false,
            seed: 0,
        };
        if !ps.spawn(Category::Precip, p) {
            break;
        }
    }
}

/// Small pieces of a broken block (glass shards, leaves, splinters, soil), thrown by the wind.
pub fn burst(ps: &mut ParticleSystem, pos: Vec3, vel: Vec3, state: &str, tornado: u32) {
    let color = block_color(state);
    let leafy = state.contains("leaves") || state.contains("grass") || state.contains("flower");
    let n = if leafy { 4 } else { 6 };
    for _ in 0..n {
        let (shape, tau, fall, size) = if leafy {
            (Shape::Leaf, 0.5, 1.5, ps.rng.range(0.15, 0.3) as f32)
        } else if state.contains("dirt") || state.contains("sand") || state.contains("gravel") {
            (Shape::Puff, 0.4, 1.0, ps.rng.range(0.6, 1.6) as f32)
        } else {
            (Shape::Chunk, 0.9, 6.0, ps.rng.range(0.1, 0.35) as f32)
        };
        let jitter = Vec3::new(ps.rng.normal(), ps.rng.normal().abs(), ps.rng.normal()) * 3.0;
        let p = Particle {
            pos: pos + jitter * 0.1,
            vel: vel + jitter,
            age: 0.0,
            life: ps.rng.range(2.5, 6.0) as f32,
            size,
            grow: 0.0,
            color,
            shape,
            tau,
            fall,
            tornado,
            condensed: false,
            seed: ps.rng.below(256) as u8,
        };
        if !ps.spawn(Category::Burst, p) {
            return;
        }
    }
}

/// A cloud-to-ground bolt: a jagged chain of bright lines from the cloud base to `ground`.
pub fn lightning(ps: &mut ParticleSystem, ground: Vec3, base_height: f64) {
    ps.flash = 1.0;
    let mut at = ground + Vec3::new(ps.rng.range(-30.0, 30.0), base_height, ps.rng.range(-30.0, 30.0));
    let segments = 10;
    for k in 0..segments {
        let remaining = (segments - k) as f64;
        let to_ground = (ground - at) / remaining;
        let next = if k == segments - 1 { ground } else { at + to_ground + Vec3::new(ps.rng.normal() * 6.0, 0.0, ps.rng.normal() * 6.0) };
        let p = Particle {
            pos: at,
            vel: next - at,
            age: 0.0,
            life: 0.35,
            size: 0.7,
            grow: 0.0,
            color: [235, 240, 255, 255],
            shape: Shape::Line,
            tau: 1e9,
            fall: 0.0,
            tornado: 0,
            condensed: false,
            seed: 0,
        };
        // a line's "velocity" is its extent: keep it from being integrated
        let _ = ps.spawn(Category::Burst, p);
        at = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::Environment;
    use crate::atmosphere::presets::Preset;
    use crate::config::{Quality, WorldScale};
    use crate::tornado::SpawnShape;

    #[test]
    fn a_tornado_makes_a_funnel_and_a_debris_cloud() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 1);
        let id = sys.spawn_tornado(SpawnShape::Stovepipe, Vec3::new(0.0, 64.0, 0.0));
        for _ in 0..600 {
            sys.step(0.05);
        }
        let mut ps = ParticleSystem::new(Quality::High, 2);
        let camera = sys.tornado_mut(id).unwrap().pos + Vec3::new(300.0, 10.0, 0.0);
        for _ in 0..60 {
            emit_all(&mut ps, &sys, camera, 0.05);
            ps.update(&sys, 0.05, camera);
        }
        let funnel = ps.particles.iter().filter(|p| p.condensed).count();
        let ground = ps.particles.iter().filter(|p| p.tornado == id && !p.condensed).count();
        assert!(funnel > 200, "{funnel}");
        assert!(ground > 50, "{ground}");
        assert!(ps.len() <= Quality::High.particle_budget());
        // they swirl: the mean angular momentum about the axis is cyclonic
        let t = sys.tornadoes.iter().find(|t| t.id == id).unwrap();
        let l: f64 = ps.particles.iter().filter(|p| p.condensed).map(|p| {
            let r = (p.pos - t.pos).horizontal();
            r.x * (-p.vel.z) - (-r.z) * p.vel.x
        }).sum();
        assert!(l > 0.0, "counter-clockwise from above");
    }

    #[test]
    fn low_quality_keeps_fewer() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 1);
        sys.spawn_tornado(SpawnShape::Wedge, Vec3::new(0.0, 64.0, 0.0));
        for _ in 0..400 {
            sys.step(0.05);
        }
        let run = |q| {
            let mut ps = ParticleSystem::new(q, 3);
            for _ in 0..80 {
                emit_all(&mut ps, &sys, Vec3::new(400.0, 70.0, 0.0), 0.05);
                ps.update(&sys, 0.05, Vec3::new(400.0, 70.0, 0.0));
            }
            ps.len()
        };
        assert!(run(Quality::Low) < run(Quality::High));
    }
}
