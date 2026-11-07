// The server half sends its weather to each player's client half over Horizon's mod messages
// (`native_net_send_to_player`, channel `stormfront:sync`, at most 30 000 bytes a message).
// Clients rebuild the storms and tornadoes from it, so their particles and radar evaluate the
// same fields the server's damage used; the server stays the only authority.
//
// Little-endian binary, versioned. Message kinds:
//   1 weather  (environment, storms, tornadoes)
//   2 debris   (flying pieces, for drawing)
//   3 effects  (bursts, tree falls, collapses, lightning, notices)
//   4 track    (a tornado's path for the trajectory view)

use crate::atmosphere::Environment;
use crate::atmosphere::presets::Preset;
use crate::atmosphere::sounding::EnvironmentSpec;
use crate::math::Vec3;
use crate::storms::{Mesocyclone, Microburst, Storm, StormKind, StormStage};
use crate::tornado::condensation::Morphology;
use crate::tornado::multivortex::{MultiVortex, SubVortex};
use crate::tornado::{RadialProfile, Tornado, TornadoStage, VortexParams};

pub const CHANNEL: &str = "stormfront:sync";
pub const VERSION: u8 = 1;
pub const MAX_MESSAGE: usize = 29_000;

pub const KIND_WEATHER: u8 = 1;
pub const KIND_DEBRIS: u8 = 2;
pub const KIND_EFFECTS: u8 = 3;
pub const KIND_TRACK: u8 = 4;

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn new(kind: u8) -> Self {
        Writer(vec![VERSION, kind])
    }
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f64) {
        self.0.extend_from_slice(&(v as f32).to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn vec3(&mut self, v: Vec3) {
        // absolute positions need more than f32's 24 bits far from the origin
        self.f64(v.x);
        self.f32(v.y);
        self.f64(v.z);
    }
    pub fn dir3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
    pub fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u16(b.len().min(u16::MAX as usize) as u16);
        self.0.extend_from_slice(&b[..b.len().min(u16::MAX as usize)]);
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, at: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub fn f32(&mut self) -> Option<f64> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().ok()?) as f64;
        v.is_finite().then_some(v)
    }
    pub fn f64(&mut self) -> Option<f64> {
        let v = f64::from_le_bytes(self.take(8)?.try_into().ok()?);
        v.is_finite().then_some(v)
    }
    pub fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f64()?, self.f32()?, self.f64()?))
    }
    pub fn dir3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

/// Header check: (kind) of a message of our version.
pub fn kind_of(data: &[u8]) -> Option<u8> {
    (data.first() == Some(&VERSION)).then(|| data.get(1).copied()).flatten()
}

fn write_env(w: &mut Writer, env: &Environment) {
    w.u8(env.preset.map_or(255, |p| Preset::ALL.iter().position(|q| *q == p).unwrap_or(255) as u8));
    let s = &env.spec;
    for v in [
        s.surface_temp_c,
        s.surface_dewpoint_c,
        s.surface_pressure_pa,
        s.boundary_lapse,
        s.mixed_layer_depth,
        s.cap_strength,
        s.mid_lapse,
        s.moist_depth,
        s.dry_aloft_depression,
        s.tropopause,
    ] {
        w.f64(v);
    }
    w.u8(s.hodograph.len().min(32) as u8);
    for &(h, sp, d) in s.hodograph.iter().take(32) {
        w.f64(h);
        w.f64(sp);
        w.f64(d);
    }
}

fn read_env(r: &mut Reader) -> Option<(Option<Preset>, EnvironmentSpec)> {
    let p = r.u8()?;
    let preset = Preset::ALL.get(p as usize).copied();
    let mut v = [0.0; 10];
    for x in &mut v {
        *x = r.f64()?;
    }
    let n = r.u8()? as usize;
    let mut hodograph = Vec::with_capacity(n);
    for _ in 0..n {
        hodograph.push((r.f64()?, r.f64()?, r.f64()?));
    }
    Some((
        preset,
        EnvironmentSpec {
            surface_temp_c: v[0],
            surface_dewpoint_c: v[1],
            surface_pressure_pa: v[2],
            boundary_lapse: v[3],
            mixed_layer_depth: v[4],
            cap_strength: v[5],
            mid_lapse: v[6],
            moist_depth: v[7],
            dry_aloft_depression: v[8],
            tropopause: v[9],
            hodograph,
        },
    ))
}

fn write_storm(w: &mut Writer, s: &Storm) {
    w.u32(s.id);
    w.u8(s.kind.id());
    w.u8(s.stage.id());
    w.vec3(s.pos);
    w.dir3(s.motion);
    for v in [s.age, s.life, s.updraft, s.updraft_radius, s.precip, s.hail, s.hail_size, s.rfd, s.ffd, s.lcl, s.cloud_base, s.top, s.rain_wrap, s.line_half_length] {
        w.f32(v);
    }
    w.dir3(s.downshear);
    w.dir3(s.line_dir);
    let m = &s.meso;
    for v in [m.strength, m.radius, m.v_rot, m.occlusion] {
        w.f32(v);
    }
    w.dir3(m.offset);
    w.u16(m.cycle.min(u16::MAX as u32) as u16);
    w.u8(s.cells.len().min(16) as u8);
    for (off, age) in s.cells.iter().take(16) {
        w.dir3(*off);
        w.f32(*age);
    }
    w.u8(s.microbursts.len().min(8) as u8);
    for mb in s.microbursts.iter().take(8) {
        w.vec3(mb.pos);
        for v in [mb.age, mb.life, mb.strength, mb.radius] {
            w.f32(v);
        }
    }
    w.u32(s.tornado.unwrap_or(0));
}

fn read_storm(r: &mut Reader, env: &Environment, scale: &crate::config::WorldScale) -> Option<Storm> {
    let id = r.u32()?;
    let kind = StormKind::from_id(r.u8()?);
    let stage = StormStage::from_id(r.u8()?);
    let pos = r.vec3()?;
    let mut s = Storm::new(id, kind, pos, env, scale, id as u64);
    s.stage = stage;
    s.motion = r.dir3()?;
    let mut v = [0.0; 14];
    for x in &mut v {
        *x = r.f32()?;
    }
    [s.age, s.life, s.updraft, s.updraft_radius, s.precip, s.hail, s.hail_size, s.rfd, s.ffd, s.lcl, s.cloud_base, s.top, s.rain_wrap, s.line_half_length] = v;
    s.downshear = r.dir3()?;
    s.line_dir = r.dir3()?;
    let (strength, radius, v_rot, occlusion) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
    let offset = r.dir3()?;
    let cycle = r.u16()? as u32;
    s.meso = Mesocyclone { strength, radius, v_rot, offset, occlusion, cycle };
    let n = r.u8()?;
    s.cells = (0..n).map(|_| Some((r.dir3()?, r.f32()?))).collect::<Option<Vec<_>>>()?;
    let n = r.u8()?;
    s.microbursts = (0..n)
        .map(|_| {
            let pos = r.vec3()?;
            Some(Microburst { pos, age: r.f32()?, life: r.f32()?, strength: r.f32()?, radius: r.f32()? })
        })
        .collect::<Option<Vec<_>>>()?;
    let t = r.u32()?;
    s.tornado = (t != 0).then_some(t);
    Some(s)
}

fn write_tornado(w: &mut Writer, t: &Tornado) {
    w.u32(t.id);
    w.u32(t.storm_id.unwrap_or(0));
    w.u32(t.satellite_of.unwrap_or(0));
    w.vec3(t.pos);
    w.dir3(t.velocity);
    let p = &t.params;
    w.f32(p.v_max);
    w.f32(p.r_max);
    match p.profile {
        RadialProfile::BurgersRott => {
            w.u8(0);
            w.f32(0.0);
        }
        RadialProfile::ModifiedRankine { alpha } => {
            w.u8(1);
            w.f32(alpha);
        }
    }
    for v in [p.inflow_ratio, p.inflow_depth, p.beta, p.surface_layer] {
        w.f32(v);
    }
    w.u8(t.stage.id());
    for v in [t.age, t.life, t.waviness, t.wave_phase, t.lcl, t.cloud_base, t.rain_wrap, t.rfd_angle, t.asymmetry, t.debris_load, t.target_v_max] {
        w.f32(v);
    }
    w.dir3(t.tilt);
    w.u8(t.funnel.morphology.id());
    w.u8(t.multivortex.enabled as u8);
    w.u8(t.multivortex.vortices.len().min(12) as u8);
    for v in t.multivortex.vortices.iter().take(12) {
        for x in [v.angle, v.orbit, v.core, v.v_peak, v.age, v.life] {
            w.f32(x);
        }
    }
}

fn read_tornado(r: &mut Reader) -> Option<Tornado> {
    let id = r.u32()?;
    let storm = r.u32()?;
    let sat = r.u32()?;
    let pos = r.vec3()?;
    let mut t = Tornado::new(id, pos, id as u64);
    t.storm_id = (storm != 0).then_some(storm);
    t.satellite_of = (sat != 0).then_some(sat);
    t.velocity = r.dir3()?;
    let v_max = r.f32()?;
    let r_max = r.f32()?;
    let profile = match (r.u8()?, r.f32()?) {
        (1, alpha) => RadialProfile::ModifiedRankine { alpha },
        _ => RadialProfile::BurgersRott,
    };
    let (inflow_ratio, inflow_depth, beta, surface_layer) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
    t.params = VortexParams { v_max, r_max, profile, inflow_ratio, inflow_depth, beta, surface_layer };
    t.stage = TornadoStage::from_id(r.u8()?);
    let mut v = [0.0; 11];
    for x in &mut v {
        *x = r.f32()?;
    }
    [t.age, t.life, t.waviness, t.wave_phase, t.lcl, t.cloud_base, t.rain_wrap, t.rfd_angle, t.asymmetry, t.debris_load, t.target_v_max] = v;
    t.tilt = r.dir3()?;
    let morph = Morphology::from_id(r.u8()?);
    let enabled = r.u8()? != 0;
    let n = r.u8()?;
    let mut mv = MultiVortex::new(enabled);
    for _ in 0..n {
        mv.vortices.push(SubVortex { angle: r.f32()?, orbit: r.f32()?, core: r.f32()?, v_peak: r.f32()?, age: r.f32()?, life: r.f32()? });
    }
    t.multivortex = mv;
    t.update_funnel(1.15);
    // the server's classification wins (it saw the same inputs)
    t.funnel.morphology = morph;
    Some(t)
}

/// Encode the weather (the server half).
pub fn encode_weather(tick: u64, env: &Environment, storms: &[Storm], tornadoes: &[Tornado], ground_y: f64) -> Vec<u8> {
    let mut w = Writer::new(KIND_WEATHER);
    w.u64(tick);
    w.f32(ground_y);
    write_env(&mut w, env);
    w.u16(storms.len().min(64) as u16);
    for s in storms.iter().take(64) {
        write_storm(&mut w, s);
    }
    w.u16(tornadoes.len().min(32) as u16);
    for t in tornadoes.iter().take(32) {
        write_tornado(&mut w, t);
    }
    w.0
}

/// Decoded weather.
pub struct WeatherMessage {
    pub tick: u64,
    pub ground_y: f64,
    pub preset: Option<Preset>,
    pub spec: EnvironmentSpec,
    pub storms: Vec<Storm>,
    pub tornadoes: Vec<Tornado>,
}

/// Decode a weather message. `env` is the client's current environment: storms are rebuilt
/// against the message's own environment when it differs (the caller swaps it in).
pub fn decode_weather(data: &[u8], env: &mut Option<Environment>, scale: &crate::config::WorldScale) -> Option<WeatherMessage> {
    let mut r = Reader::new(data);
    if r.u8()? != VERSION || r.u8()? != KIND_WEATHER {
        return None;
    }
    let tick = r.u64()?;
    let ground_y = r.f32()?;
    let (preset, spec) = read_env(&mut r)?;
    let same = env.as_ref().is_some_and(|e| e.spec == spec);
    if !same {
        let mut e = Environment::from_spec(spec.clone());
        e.preset = preset;
        *env = Some(e);
    }
    let e = env.as_ref()?;
    let n = r.u16()?;
    let storms = (0..n).map(|_| read_storm(&mut r, e, scale)).collect::<Option<Vec<_>>>()?;
    let n = r.u16()?;
    let tornadoes = (0..n).map(|_| read_tornado(&mut r)).collect::<Option<Vec<_>>>()?;
    Some(WeatherMessage { tick, ground_y, preset, spec, storms, tornadoes })
}

/// A flying debris piece as clients draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct DebrisView {
    pub id: u32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub size: f64,
    pub state: String,
}

/// Encode debris in as many messages as needed.
pub fn encode_debris(tick: u64, pieces: &[(u32, Vec3, Vec3, f64, &str)]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in pieces.chunks(450) {
        let mut w = Writer::new(KIND_DEBRIS);
        w.u64(tick);
        // a table of the block states in this message
        let mut table: Vec<&str> = Vec::new();
        let idx: Vec<u8> = chunk
            .iter()
            .map(|p| match table.iter().position(|s| *s == p.4) {
                Some(i) => i as u8,
                None if table.len() < 255 => {
                    table.push(p.4);
                    (table.len() - 1) as u8
                }
                None => 0,
            })
            .collect();
        w.u8(table.len() as u8);
        for s in &table {
            w.str(s);
        }
        w.u16(chunk.len() as u16);
        for (p, i) in chunk.iter().zip(idx) {
            w.u32(p.0);
            w.vec3(p.1);
            w.dir3(p.2);
            w.f32(p.3);
            w.u8(i);
        }
        out.push(w.0);
    }
    if out.is_empty() {
        let mut w = Writer::new(KIND_DEBRIS);
        w.u64(tick);
        w.u8(0);
        w.u16(0);
        out.push(w.0);
    }
    out
}

pub fn decode_debris(data: &[u8]) -> Option<(u64, Vec<DebrisView>)> {
    let mut r = Reader::new(data);
    if r.u8()? != VERSION || r.u8()? != KIND_DEBRIS {
        return None;
    }
    let tick = r.u64()?;
    let n = r.u8()?;
    let table = (0..n).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
    let n = r.u16()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let id = r.u32()?;
        let pos = r.vec3()?;
        let vel = r.dir3()?;
        let size = r.f32()?;
        let i = r.u8()? as usize;
        out.push(DebrisView { id, pos, vel, size, state: table.get(i).cloned().unwrap_or_default() });
    }
    Some((tick, out))
}

/// Things clients show or play once.
#[derive(Clone, Debug, PartialEq)]
pub enum Fx {
    Burst { pos: Vec3, vel: Vec3, state: String },
    TreeFall { pos: Vec3 },
    Collapse { pos: Vec3, blocks: u32 },
    Fragment { pos: Vec3, pieces: u32, state: String },
    Lightning { pos: Vec3 },
    /// A line for the chat (tornado warnings, formations).
    Notice { text: String },
}

pub fn encode_effects(tick: u64, fx: &[Fx]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut w = Writer::new(KIND_EFFECTS);
    w.u64(tick);
    let mut count_at = w.len();
    w.u16(0);
    let mut count = 0u16;
    for f in fx {
        if w.len() > MAX_MESSAGE - 300 {
            w.0[count_at..count_at + 2].copy_from_slice(&count.to_le_bytes());
            out.push(std::mem::take(&mut w.0));
            w = Writer::new(KIND_EFFECTS);
            w.u64(tick);
            count_at = w.len();
            w.u16(0);
            count = 0;
        }
        match f {
            Fx::Burst { pos, vel, state } => {
                w.u8(1);
                w.vec3(*pos);
                w.dir3(*vel);
                w.str(state);
            }
            Fx::TreeFall { pos } => {
                w.u8(2);
                w.vec3(*pos);
            }
            Fx::Collapse { pos, blocks } => {
                w.u8(3);
                w.vec3(*pos);
                w.u32(*blocks);
            }
            Fx::Fragment { pos, pieces, state } => {
                w.u8(4);
                w.vec3(*pos);
                w.u32(*pieces);
                w.str(state);
            }
            Fx::Lightning { pos } => {
                w.u8(5);
                w.vec3(*pos);
            }
            Fx::Notice { text } => {
                w.u8(6);
                w.str(text);
            }
        }
        count += 1;
    }
    w.0[count_at..count_at + 2].copy_from_slice(&count.to_le_bytes());
    if count > 0 {
        out.push(w.0);
    }
    out
}

pub fn decode_effects(data: &[u8]) -> Option<(u64, Vec<Fx>)> {
    let mut r = Reader::new(data);
    if r.u8()? != VERSION || r.u8()? != KIND_EFFECTS {
        return None;
    }
    let tick = r.u64()?;
    let n = r.u16()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(match r.u8()? {
            1 => Fx::Burst { pos: r.vec3()?, vel: r.dir3()?, state: r.str()? },
            2 => Fx::TreeFall { pos: r.vec3()? },
            3 => Fx::Collapse { pos: r.vec3()?, blocks: r.u32()? },
            4 => Fx::Fragment { pos: r.vec3()?, pieces: r.u32()?, state: r.str()? },
            5 => Fx::Lightning { pos: r.vec3()? },
            6 => Fx::Notice { text: r.str()? },
            _ => return None,
        });
    }
    Some((tick, out))
}

/// A tornado's path: (x, z, v_max) points.
pub fn encode_track(id: u32, points: &[(f64, f64, f64)]) -> Vec<u8> {
    let mut w = Writer::new(KIND_TRACK);
    w.u32(id);
    let n = points.len().min(2000);
    let step = points.len().div_ceil(n.max(1)).max(1);
    let pts: Vec<_> = points.iter().step_by(step).collect();
    w.u16(pts.len() as u16);
    for p in pts {
        w.f64(p.0);
        w.f64(p.1);
        w.f32(p.2);
    }
    w.0
}

pub fn decode_track(data: &[u8]) -> Option<(u32, Vec<(f64, f64, f64)>)> {
    let mut r = Reader::new(data);
    if r.u8()? != VERSION || r.u8()? != KIND_TRACK {
        return None;
    }
    let id = r.u32()?;
    let n = r.u16()?;
    let pts = (0..n).map(|_| Some((r.f64()?, r.f64()?, r.f32()?))).collect::<Option<Vec<_>>>()?;
    Some((id, pts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorldScale;
    use crate::storms::system::StormSystem;
    use crate::tornado::SpawnShape;

    #[test]
    fn weather_round_trips_and_fits_a_message() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 1);
        sys.spawn_tornado(SpawnShape::MultipleVortex, Vec3::new(1_000_123.5, 64.0, -2_000_456.25));
        for _ in 0..400 {
            sys.step(0.05);
        }
        let data = encode_weather(77, &sys.env, &sys.storms, &sys.tornadoes, 64.0);
        assert!(data.len() < MAX_MESSAGE, "{}", data.len());
        let mut env = None;
        let m = decode_weather(&data, &mut env, &WorldScale::default()).unwrap();
        assert_eq!(m.tick, 77);
        assert_eq!(m.storms.len(), sys.storms.len());
        assert_eq!(m.tornadoes.len(), sys.tornadoes.len());
        let (a, b) = (&sys.tornadoes[0], &m.tornadoes[0]);
        assert!((a.pos.x - b.pos.x).abs() < 1e-6 && (a.pos.z - b.pos.z).abs() < 1e-6, "f64 positions survive");
        assert!((a.params.v_max - b.params.v_max).abs() < 1e-3);
        assert_eq!(a.multivortex.vortices.len(), b.multivortex.vortices.len());
        // the client evaluates the same wind
        let p = a.pos + Vec3::new(a.params.r_max, 10.0, 0.0);
        let (wa, wb) = (a.wind_at(p), b.wind_at(p));
        assert!((wa - wb).length() < 0.05, "{wa:?} {wb:?}");
        assert_eq!(env.unwrap().spec, sys.env.spec);
    }

    #[test]
    fn debris_and_effects_round_trip() {
        let pieces: Vec<(u32, Vec3, Vec3, f64, &str)> = (0..1000).map(|i| (i, Vec3::new(i as f64, 70.0, 0.0), Vec3::new(1.0, 2.0, 3.0), 0.8, if i % 2 == 0 { "minecraft:oak_planks" } else { "minecraft:bricks" })).collect();
        let msgs = encode_debris(5, &pieces);
        assert!(msgs.len() >= 2 && msgs.iter().all(|m| m.len() < 30_000));
        let all: Vec<DebrisView> = msgs.iter().flat_map(|m| decode_debris(m).unwrap().1).collect();
        assert_eq!(all.len(), 1000);
        assert_eq!(all[3].state, "minecraft:bricks");

        let fx: Vec<Fx> = (0..2000).map(|i| Fx::Burst { pos: Vec3::new(i as f64, 0.0, 0.0), vel: Vec3::ZERO, state: "minecraft:glass".into() }).chain([Fx::Notice { text: "Tornado!".into() }]).collect();
        let msgs = encode_effects(9, &fx);
        assert!(msgs.iter().all(|m| m.len() < 30_000));
        let back: Vec<Fx> = msgs.iter().flat_map(|m| decode_effects(m).unwrap().1).collect();
        assert_eq!(back.len(), fx.len());
        assert_eq!(back.last(), fx.last());
    }

    #[test]
    fn garbage_is_rejected() {
        let mut env = None;
        assert!(decode_weather(&[1, 1, 0, 0], &mut env, &WorldScale::default()).is_none());
        assert!(decode_effects(&[9, 3]).is_none());
        assert!(kind_of(&[VERSION, KIND_TRACK]) == Some(KIND_TRACK));
        let t = encode_track(3, &[(1.0, 2.0, 50.0), (3.0, 4.0, 60.0)]);
        assert_eq!(decode_track(&t).unwrap(), (3, vec![(1.0, 2.0, 50.0), (3.0, 4.0, 60.0)]));
    }
}
