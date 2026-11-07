// The damage engine: each server tick it reads the world around tornadoes and downbursts in
// whole chunks (within a budget), evaluates the chunks that most need it (strongest winds,
// longest since last evaluated), applies trees, scouring and block failures with the elapsed
// time of each chunk, finds collapses, moves debris and writes the changes back in batches.
//
// Its physics depends only on the wind field and the world, never on any client's particle
// settings.

use super::debris::{DebrisEvent, DebrisSystem};
use super::ground_scouring::{ScourEdit, ScourState, step_column};
use super::materials::{Kind, Species};
use super::structures::{DamageKind, check, classify_failure, unsupported_after};
use super::survey::TornadoRecord;
use super::vegetation::{TreeOutcome, TreeState, count_leaves, debark_rate, felled_trunk, find_trunk, step_tree, stripped};
use super::world::{READ_ABOVE, READ_BELOW, VoxelWindow, WorldIo};
use crate::config::ServerSettings;
use crate::math::{Rng, Vec3, hash01};
use crate::storms::system::StormSystem;
use std::collections::HashMap;

/// A visible effect of damage for clients (particles and sounds); not physics.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// A block broke into small pieces (glass shards, leaves, soil, splinters).
    Burst { pos: Vec3, state: String, vel: Vec3 },
    TreeFall { pos: Vec3 },
    Collapse { pos: Vec3, blocks: u32 },
    Fragment { pos: Vec3, state: String, pieces: u32 },
}

/// What a tick did, for logs and the debug screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TickReport {
    pub columns_read: usize,
    pub columns_evaluated: usize,
    pub blocks_checked: usize,
    pub blocks_failed: usize,
    pub edits_flushed: u64,
}

/// Per-tick work limits (columns), set from how fast the host is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub read_chunks: usize,
    pub eval_columns: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { read_chunks: 6, eval_columns: 3072 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ChunkState {
    read_at: f64,
    evaluated_at: f64,
    /// Read failed (not loaded): retry later.
    missing_until: f64,
}

/// A region the engine works on this tick.
#[derive(Clone, Copy, Debug)]
struct Footprint {
    center: Vec3,
    radius: f64,
    /// The tornado whose survey records damage here (None: a downburst).
    tornado: Option<u32>,
    peak: f64,
}

pub struct DamageEngine {
    pub window: VoxelWindow,
    pub debris: DebrisSystem,
    pub effects: Vec<Effect>,
    /// Damage done by downbursts (straight-line winds), outside any tornado's survey.
    pub downburst: TornadoRecord,
    chunks: HashMap<(i32, i32), ChunkState>,
    fatigue: HashMap<[i32; 3], f32>,
    trees: HashMap<[i32; 3], TreeState>,
    scour: HashMap<(i32, i32), ScourState>,
    abrasion: HashMap<[i32; 3], f32>,
    pub time: f64,
    ticks: u64,
    rng: Rng,
    pub last: TickReport,
}

const EFFECT_CAP: usize = 256;

impl DamageEngine {
    pub fn new(settings: &ServerSettings, seed: u64) -> Self {
        DamageEngine {
            window: VoxelWindow::new(),
            debris: DebrisSystem::new(settings.max_debris, seed),
            effects: Vec::new(),
            downburst: TornadoRecord::new(0, 0, 0),
            chunks: HashMap::new(),
            fatigue: HashMap::new(),
            trees: HashMap::new(),
            scour: HashMap::new(),
            abrasion: HashMap::new(),
            time: 0.0,
            ticks: 0,
            rng: Rng::new(seed ^ 0xDA3A6E),
            last: TickReport::default(),
        }
    }

    fn effect(&mut self, e: Effect) {
        if self.effects.len() < EFFECT_CAP {
            self.effects.push(e);
        }
    }

    fn footprints(sys: &StormSystem, settings: &ServerSettings) -> Vec<Footprint> {
        let mut out = Vec::new();
        for t in &sys.tornadoes {
            if t.params.v_max < 18.0 {
                continue;
            }
            let r = (t.radius_of_wind(26.0) + settings.damage_margin).clamp(16.0, 480.0);
            out.push(Footprint { center: t.pos, radius: r, tornado: Some(t.id), peak: t.peak_wind() });
        }
        for s in &sys.storms {
            for m in &s.microbursts {
                let peak = m.strength * m.envelope();
                if peak > 26.0 {
                    out.push(Footprint { center: m.pos, radius: (m.radius * 2.2).min(480.0), tornado: None, peak });
                }
            }
        }
        out
    }

    /// One server tick of damage.
    pub fn tick(&mut self, io: &mut dyn WorldIo, sys: &mut StormSystem, records: &mut HashMap<u32, TornadoRecord>, dt: f64, settings: &ServerSettings, budget: Budget) -> TickReport {
        self.time += dt;
        self.ticks += 1;
        let mut report = TickReport::default();
        let feet = Self::footprints(sys, settings);
        let rho = sys.env.density(0.0);
        if settings.damage && !feet.is_empty() {
            report.columns_read = self.read_chunks(io, &feet, budget.read_chunks);
            let chosen = self.choose_chunks(&feet, budget.eval_columns / 256 + 1);
            for (cx, cz, fp) in chosen {
                let st = self.chunks.entry((cx, cz)).or_default();
                let cdt = (self.time - st.evaluated_at).clamp(dt, 2.0);
                st.evaluated_at = self.time;
                let wet = wetness(sys, Vec3::new(cx as f64 * 16.0 + 8.0, 0.0, cz as f64 * 16.0 + 8.0));
                let mut removed = Vec::new();
                for dx in 0..16 {
                    for dz in 0..16 {
                        let (x, z) = (cx * 16 + dx, cz * 16 + dz);
                        let d = Vec3::new(x as f64 + 0.5 - fp.center.x, 0.0, z as f64 + 0.5 - fp.center.z).horizontal_length();
                        if d > fp.radius {
                            continue;
                        }
                        if self.window.column(x, z).is_none() {
                            continue;
                        }
                        report.columns_evaluated += 1;
                        self.column(sys, records, &fp, x, z, cdt, wet, rho, &mut removed, &mut report);
                    }
                }
                if !removed.is_empty() {
                    self.collapse(sys, records, &fp, &removed);
                }
            }
        }
        // debris flies whether or not new damage happens
        let ground_y = sys.ground_y;
        {
            let sysr: &StormSystem = sys;
            let wind = |p: Vec3| sysr.wind_at(p);
            self.debris.step(dt, &mut self.window, rho, ground_y, &wind);
        }
        self.debris_events(records);
        // lofted debris feeds each tornado's debris cloud (and the radar's debris signature)
        for t in &mut sys.tornadoes {
            let lofted = self.debris.lofted_mass(t.pos, (t.params.r_max * 3.0).max(60.0), t.pos.y);
            t.debris_load = t.debris_load.max(lofted);
        }
        if self.ticks % settings.flush_interval_ticks.max(1) as u64 == 0 && self.window.pending_edits() > 0 {
            let edits = self.window.take_edits(settings.max_edits_per_flush);
            match io.edit_blocks(&edits) {
                Ok(n) => report.edits_flushed = n,
                Err(_) => {
                    // the level is not the one being ticked: keep them for the next flush
                    for (p, s) in edits {
                        self.window.set_state(p, &s);
                    }
                }
            }
        }
        if self.ticks % 100 == 0 {
            let keep: Vec<(f64, f64)> = sys.tornadoes.iter().map(|t| (t.pos.x, t.pos.z)).chain(feet.iter().map(|f| (f.center.x, f.center.z))).collect();
            let stale = self.time - 45.0;
            self.window.evict(&keep, 560.0, stale);
            let window = &self.window;
            self.chunks.retain(|&(cx, cz), _| window.has_column(cx * 16, cz * 16) || window.has_column(cx * 16 + 15, cz * 16 + 15));
            self.fatigue.retain(|_, f| *f > 0.02);
            if self.fatigue.len() > 200_000 {
                self.fatigue.clear();
            }
            self.abrasion.retain(|_, a| *a > 0.05);
        }
        self.last = report;
        report
    }

    /// Read the nearest unread (or stale) chunks inside the footprints.
    fn read_chunks(&mut self, io: &mut dyn WorldIo, feet: &[Footprint], max_chunks: usize) -> usize {
        let mut wanted: Vec<(f64, (i32, i32))> = Vec::new();
        for f in feet {
            let r = f.radius + 16.0;
            let (c0x, c1x) = (((f.center.x - r) / 16.0).floor() as i32, ((f.center.x + r) / 16.0).floor() as i32);
            let (c0z, c1z) = (((f.center.z - r) / 16.0).floor() as i32, ((f.center.z + r) / 16.0).floor() as i32);
            for cx in c0x..=c1x {
                for cz in c0z..=c1z {
                    let c = Vec3::new(cx as f64 * 16.0 + 8.0, 0.0, cz as f64 * 16.0 + 8.0);
                    let d = (c - f.center).horizontal_length();
                    if d > r + 12.0 {
                        continue;
                    }
                    let st = self.chunks.get(&(cx, cz)).copied().unwrap_or_default();
                    let unread = st.read_at <= 0.0;
                    let stale = self.time - st.read_at > 30.0;
                    if (unread || stale) && self.time >= st.missing_until {
                        // nearest first, and unread before stale
                        wanted.push((d + if unread { 0.0 } else { 400.0 }, (cx, cz)));
                    }
                }
            }
        }
        wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
        wanted.dedup_by_key(|w| w.1);
        let mut read = 0;
        for &(_, (cx, cz)) in wanted.iter().take(max_chunks) {
            let cols: Vec<(i32, i32)> = (0..16).flat_map(|dx| (0..16).map(move |dz| (cx * 16 + dx, cz * 16 + dz))).collect();
            match io.read_columns(&cols, READ_BELOW, READ_ABOVE) {
                Ok(data) => {
                    let loaded = data.bases.iter().any(|b| b.is_some());
                    self.window.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, self.time);
                    let st = self.chunks.entry((cx, cz)).or_default();
                    if loaded {
                        st.read_at = self.time;
                        if st.evaluated_at <= 0.0 {
                            st.evaluated_at = self.time;
                        }
                        read += cols.len();
                    } else {
                        st.missing_until = self.time + 5.0;
                    }
                }
                Err(_) => {
                    self.chunks.entry((cx, cz)).or_default().missing_until = self.time + 5.0;
                }
            }
        }
        read
    }

    /// The chunks to evaluate this tick: most overdue, weighted by how strong the wind is.
    fn choose_chunks(&self, feet: &[Footprint], max: usize) -> Vec<(i32, i32, Footprint)> {
        let mut cand: Vec<(f64, i32, i32, Footprint)> = Vec::new();
        for f in feet {
            let r = f.radius;
            let (c0x, c1x) = (((f.center.x - r) / 16.0).floor() as i32, ((f.center.x + r) / 16.0).floor() as i32);
            let (c0z, c1z) = (((f.center.z - r) / 16.0).floor() as i32, ((f.center.z + r) / 16.0).floor() as i32);
            for cx in c0x..=c1x {
                for cz in c0z..=c1z {
                    let Some(st) = self.chunks.get(&(cx, cz)) else { continue };
                    if st.read_at <= 0.0 {
                        continue;
                    }
                    let c = Vec3::new(cx as f64 * 16.0 + 8.0, 0.0, cz as f64 * 16.0 + 8.0);
                    let d = (c - f.center).horizontal_length();
                    if d > r + 12.0 {
                        continue;
                    }
                    let near = 1.0 + 3.0 * (1.0 - d / r.max(1.0)).max(0.0) * (f.peak / 60.0);
                    let overdue = self.time - st.evaluated_at;
                    cand.push((overdue * near * near, cx, cz, *f));
                }
            }
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0));
        cand.dedup_by_key(|c| (c.1, c.2));
        cand.into_iter().take(max).map(|c| (c.1, c.2, c.3)).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn column(&mut self, sys: &StormSystem, records: &mut HashMap<u32, TornadoRecord>, fp: &Footprint, x: i32, z: i32, dt: f64, wet: f64, rho: f64, removed: &mut Vec<[i32; 3]>, report: &mut TickReport) {
        let Some(col) = self.window.column(x, z) else { return };
        let g = col.ground;
        let top = self.window.top(x, z).unwrap_or(g);
        let (xf, zf) = (x as f64 + 0.5, z as f64 + 0.5);
        // wind at a few heights above the ground, interpolated for each block
        let heights = [1.5, 5.0, 12.0, 28.0];
        let samples: [Vec3; 4] = heights.map(|h| sys.wind_at(Vec3::new(xf, g as f64 + h, zf)));
        let peak = samples.iter().map(|v| v.length()).fold(0.0, f64::max);
        let wind_at = |y: f64| -> Vec3 {
            let h = y - g as f64;
            if h <= heights[0] {
                return samples[0];
            }
            for i in 0..3 {
                if h <= heights[i + 1] {
                    let t = (h - heights[i]) / (heights[i + 1] - heights[i]);
                    return samples[i].lerp(samples[i + 1], t);
                }
            }
            samples[3]
        };
        // the survey counts what stood here the first time the tornado reached it
        let tid = fp.tornado.unwrap_or(0);
        if tid != 0 && col.observed_by != tid {
            let n = (g + 1..=top).filter(|&y| self.window.material([x, y, z]).is_some_and(|m| m.is_structural())).count() as u32;
            if let Some(c) = self.window.column_mut(x, z) {
                c.observed_by = tid;
            }
            if n > 0 {
                record(records, &mut self.downburst, fp.tornado).observed_structure(x, z, n);
            }
        }
        if peak < 24.0 {
            return;
        }
        // ground scouring
        let surface = samples[0].horizontal_length();
        let st = self.scour.entry((x, z)).or_default();
        let (edits, stage) = step_column(&self.window, st, x, z, surface, wet, dt);
        for e in edits {
            match e {
                ScourEdit::Replace(p, s) => {
                    self.window.set_state(p, s);
                }
                ScourEdit::Remove(p) => {
                    let state = self.window.state(p).unwrap_or("minecraft:dirt").to_string();
                    self.window.remove(p);
                    self.effect(Effect::Burst { pos: Vec3::new(xf, p[1] as f64 + 0.5, zf), state, vel: samples[0] * 0.5 });
                }
            }
        }
        if let Some(stage) = stage {
            self.window.refresh_ground(x, z);
            let rec = record(records, &mut self.downburst, fp.tornado);
            rec.scour(x, z, stage);
            let kind = match stage {
                1 | 2 => DamageKind::ScourLight,
                3 => DamageKind::ScourModerate,
                _ => DamageKind::ScourExtreme,
            };
            if stage >= 2 || stage == 1 {
                rec.damage([x, g, z], kind, surface);
            }
        }
        // a tree standing here
        if let Some(trunk) = find_trunk(&self.window, x, z) {
            self.tree(records, fp, &trunk, &wind_at, dt, wet);
        }
        // every exposed block, top down
        let gust_phase = (self.ticks / 4) as u32;
        let mut y = top;
        while y >= g - 1 {
            let p = [x, y, z];
            y -= 1;
            let Some(id) = self.window.get(p) else { continue };
            let m = *self.window.palette.material(id);
            if m.is_air() || m.kind == Kind::Fluid || m.kind == Kind::Unbreakable || m.terrain {
                continue;
            }
            if matches!(m.kind, Kind::Log | Kind::StrippedLog) && find_trunk(&self.window, x, z).is_some_and(|t| p[1] >= t.base[1] && p[1] < t.base[1] + t.height) {
                continue; // trees are handled whole
            }
            let mut m = m;
            if matches!(m.kind, Kind::Log | Kind::StrippedLog) {
                // logs built into something: like heavy timber
                m.strength = 2600.0;
                m.connect = 0.5;
                m.kind = Kind::Wood;
            }
            let gust = 1.0 + 0.16 * (hash01(x, p[1], z, gust_phase) - 0.5);
            let w = wind_at(p[1] as f64 + 0.5) * gust;
            report.blocks_checked += 1;
            // attachments fall with what they hang on
            if m.kind == Kind::Attachment {
                let held = super::structures::FACES.iter().any(|f| self.window.material([p[0] + f[0], p[1] + f[1], p[2] + f[2]]).is_some_and(|n| n.is_solid()));
                if !held {
                    self.window.remove(p);
                    continue;
                }
            }
            let lc = check(&self.window, p, &m, w, rho);
            if lc.exposed == 0 {
                continue;
            }
            let ratio = lc.load / lc.resistance.max(1.0);
            let f = self.fatigue.entry(p).or_insert(0.0);
            if ratio > 0.65 {
                *f = (*f + (dt * 0.08 * (ratio - 0.65) / 0.35) as f32).min(1.0);
            }
            let effective = lc.resistance * (1.0 - 0.35 * *f as f64);
            if lc.load < effective {
                continue;
            }
            // it fails
            report.blocks_failed += 1;
            let state = self.window.palette.state(id).to_string();
            let kind = classify_failure(&m, &state, &lc, p[1] - g);
            self.fatigue.remove(&p);
            self.window.remove(p);
            let rec = record(records, &mut self.downburst, fp.tornado);
            rec.damage(p, kind, w.length());
            if m.kind == Kind::Leaves {
                let soft = Species::of(&state).is_softwood();
                rec.tree(p, soft, 0, w.length());
            }
            if m.is_structural() || matches!(m.kind, Kind::Wood) {
                removed.push(p);
            }
            let launched = m.debris && self.debris.launch(p, &state, w, fp.tornado);
            if launched {
                rec.debris_launched += 1;
            } else {
                self.effect(Effect::Burst { pos: Vec3::new(xf, p[1] as f64 + 0.5, zf), state, vel: w * 0.6 });
            }
            // small debris and shards add to the lofted load for the radar
            if let Some(t) = fp.tornado {
                let _ = t;
            }
        }
    }

    fn tree(&mut self, records: &mut HashMap<u32, TornadoRecord>, fp: &Footprint, trunk: &super::vegetation::Trunk, wind_at: &dyn Fn(f64) -> Vec3, dt: f64, wet: f64) {
        let top_y = trunk.base[1] + trunk.height;
        let crown = wind_at(top_y as f64);
        let u = crown.horizontal_length();
        let soft = trunk.species.is_softwood();
        let state = self.trees.entry(trunk.base).or_default();
        if state.gone {
            return;
        }
        let leaves = count_leaves(&self.window, trunk);
        let outcome = step_tree(state, trunk, leaves, u, wet, dt, &mut self.rng);
        let stripped_trunk = self.window.state(trunk.base).is_some_and(|s| s.contains("stripped_"));
        match outcome {
            TreeOutcome::Standing => {
                // debarking of the standing trunk, block by block
                for k in 0..trunk.height {
                    let p = [trunk.base[0], trunk.base[1] + k, trunk.base[2]];
                    let uh = wind_at(p[1] as f64).horizontal_length();
                    let ab = self.abrasion.get(&p).copied().unwrap_or(0.0) as f64;
                    let rate = debark_rate(uh, ab);
                    if rate > 0.0 && self.rng.chance((rate * dt).min(1.0)) {
                        if let Some(s) = self.window.state(p).and_then(stripped) {
                            self.window.set_state(p, &s);
                            let rec = record(records, &mut self.downburst, fp.tornado);
                            rec.damage(p, DamageKind::Debarked, uh);
                            rec.tree(p, soft, 3, uh);
                        }
                    }
                }
            }
            TreeOutcome::Snapped { at } => {
                let removed_logs = top_y - at[1];
                self.fell(trunk, at[1], crown);
                let rec = record(records, &mut self.downburst, fp.tornado);
                rec.damage(at, DamageKind::TreeSnapped, u);
                rec.tree(at, soft, 2, u);
                if u < 75.0 {
                    for (p, s) in felled_trunk(&self.window, at, removed_logs, trunk.species, stripped_trunk, crown) {
                        self.window.set_state(p, &s);
                    }
                } else {
                    for k in 0..removed_logs.min(4) {
                        let p = [at[0], at[1] + k, at[2]];
                        let s = format!("minecraft:{}_log[axis=y]", trunk.species.prefix());
                        self.debris.launch(p, &s, crown, fp.tornado);
                    }
                }
                self.effect(Effect::TreeFall { pos: Vec3::new(at[0] as f64 + 0.5, at[1] as f64, at[2] as f64 + 0.5) });
            }
            TreeOutcome::Uprooted => {
                self.fell(trunk, trunk.base[1], crown);
                let b = trunk.base;
                self.window.set_state([b[0], b[1] - 1, b[2]], "minecraft:rooted_dirt");
                let rec = record(records, &mut self.downburst, fp.tornado);
                rec.damage(b, DamageKind::TreeUprooted, u);
                rec.tree(b, soft, 1, u);
                for (p, s) in felled_trunk(&self.window, [b[0], b[1] - 1, b[2]], trunk.height, trunk.species, stripped_trunk, crown) {
                    self.window.set_state(p, &s);
                }
                self.effect(Effect::TreeFall { pos: Vec3::new(b[0] as f64 + 0.5, b[1] as f64, b[2] as f64 + 0.5) });
                if let Some(st) = self.trees.get_mut(&trunk.base) {
                    st.gone = true;
                }
            }
        }
    }

    /// Remove a trunk from height `from` up, with its crown.
    fn fell(&mut self, trunk: &super::vegetation::Trunk, from: i32, wind: Vec3) {
        let top = trunk.base[1] + trunk.height;
        for y in from..top {
            self.window.remove([trunk.base[0], y, trunk.base[2]]);
        }
        for dx in -3..=3 {
            for dz in -3..=3 {
                for y in (top - 4).max(from)..=(top + 3) {
                    let p = [trunk.base[0] + dx, y, trunk.base[2] + dz];
                    if self.window.material(p).is_some_and(|m| m.kind == Kind::Leaves) {
                        let s = self.window.state(p).unwrap_or("minecraft:oak_leaves").to_string();
                        self.window.remove(p);
                        if (dx + dz + y) % 3 == 0 {
                            self.effect(Effect::Burst { pos: Vec3::new(p[0] as f64 + 0.5, y as f64 + 0.5, p[2] as f64 + 0.5), state: s, vel: wind * 0.7 });
                        }
                    }
                }
            }
        }
    }

    fn collapse(&mut self, sys: &StormSystem, records: &mut HashMap<u32, TornadoRecord>, fp: &Footprint, removed: &[[i32; 3]]) {
        let falling = unsupported_after(&self.window, removed, 4000);
        if falling.is_empty() {
            return;
        }
        let mut center = Vec3::ZERO;
        for p in &falling {
            let state = self.window.state(*p).unwrap_or("minecraft:air").to_string();
            self.window.remove(*p);
            let at = Vec3::new(p[0] as f64 + 0.5, p[1] as f64 + 0.5, p[2] as f64 + 0.5);
            center += at;
            let w = sys.wind_at(at);
            let rec = record(records, &mut self.downburst, fp.tornado);
            rec.damage(*p, DamageKind::Collapse, w.length());
            if !self.debris.launch(*p, &state, w * 0.5, fp.tornado) {
                self.effect(Effect::Burst { pos: at, state, vel: w * 0.3 });
            }
        }
        let n = falling.len() as u32;
        self.effect(Effect::Collapse { pos: center / n as f64, blocks: n });
    }

    fn debris_events(&mut self, records: &mut HashMap<u32, TornadoRecord>) {
        let events = std::mem::take(&mut self.debris.events);
        for e in events {
            match e {
                DebrisEvent::Broke { pos, kind, energy } => {
                    let v = (2.0 * energy / 50.0).sqrt();
                    let rec = nearest_record(records, &mut self.downburst, pos);
                    rec.damage(pos, kind, v);
                    rec.damage(pos, DamageKind::DebrisImpact, v);
                }
                DebrisEvent::Abraded { pos } => {
                    *self.abrasion.entry(pos).or_insert(0.0) += 0.25;
                }
                DebrisEvent::Fragmented { pos, pieces, state } => {
                    for r in records.values_mut() {
                        if r.dissipated_tick.is_none() {
                            r.debris_fragmented += 1;
                            break;
                        }
                    }
                    self.effect(Effect::Fragment { pos, state, pieces });
                }
                DebrisEvent::Landed { distance, .. } => {
                    for r in records.values_mut() {
                        if r.dissipated_tick.is_none() {
                            r.debris_max_distance = r.debris_max_distance.max(distance);
                        }
                    }
                }
            }
        }
    }
}

fn record<'a>(records: &'a mut HashMap<u32, TornadoRecord>, downburst: &'a mut TornadoRecord, tornado: Option<u32>) -> &'a mut TornadoRecord {
    match tornado {
        Some(id) => records.entry(id).or_insert_with(|| TornadoRecord::new(id, 0, 0)),
        None => downburst,
    }
}

/// The record of the active tornado nearest to `pos` (by its last track point).
fn nearest_record<'a>(records: &'a mut HashMap<u32, TornadoRecord>, downburst: &'a mut TornadoRecord, pos: [i32; 3]) -> &'a mut TornadoRecord {
    let p = Vec3::new(pos[0] as f64, 0.0, pos[2] as f64);
    let id = records
        .iter()
        .filter(|(_, r)| r.dissipated_tick.is_none())
        .filter_map(|(id, r)| r.track.last().map(|t| (*id, (t.pos - p).horizontal_length())))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|a| a.0);
    match id {
        Some(id) => records.get_mut(&id).unwrap(),
        None => downburst,
    }
}

/// Ground wetness (0..1) from the rain falling there now.
pub fn wetness(sys: &StormSystem, at: Vec3) -> f64 {
    let rain: f64 = sys.storms.iter().map(|s| s.hydrometeors(at, 500.0).rain).sum();
    (rain / 2.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::super::world::testing::MemoryWorld;
    use super::*;
    use crate::atmosphere::Environment;
    use crate::atmosphere::presets::Preset;
    use crate::config::WorldScale;
    use crate::tornado::SpawnShape;

    fn run(shape: SpawnShape, v: f64, seconds: f64, world: &mut MemoryWorld, at: Vec3) -> (HashMap<u32, TornadoRecord>, DamageEngine) {
        let settings = ServerSettings::default();
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 3);
        sys.ground_y = world.ground as f64;
        let id = sys.spawn_tornado(shape, at);
        {
            let t = sys.tornado_mut(id).unwrap();
            t.overrides.v_max = Some(v);
            t.overrides.speed = Some(8.0);
            t.params.v_max = v;
            t.params.r_max = 40.0;
            t.target_r_max = 40.0;
            t.velocity = Vec3::new(8.0, 0.0, 0.0);
        }
        let mut engine = DamageEngine::new(&settings, 1);
        let mut records = HashMap::new();
        records.insert(id, TornadoRecord::new(id, 0, 0));
        let budget = Budget { read_chunks: 16, eval_columns: 4096 };
        let n = (seconds * 20.0) as usize;
        for _ in 0..n {
            sys.step(0.05);
            if let Some(t) = sys.tornado_mut(id) {
                t.pos.y = 64.0;
                t.params.r_max = 40.0;
            }
            engine.tick(world, &mut sys, &mut records, 0.05, &settings, budget);
        }
        // flush what is left
        let edits = engine.window.take_edits(usize::MAX);
        world.edit_blocks(&edits).unwrap();
        (records, engine)
    }

    #[test]
    fn a_weak_tornado_breaks_windows_and_roofs_but_not_walls() {
        let mut world = MemoryWorld::flat(64);
        world.house(-4, -3);
        // an EF1-like vortex: about 33 m/s rotation plus 8 m/s translation
        let (records, _) = run(SpawnShape::Cone, 33.0, 8.0, &mut world, Vec3::new(-30.0, 64.0, 0.0));
        let r = records.values().next().unwrap();
        let walls = r.kinds[DamageKind::Wall.index()];
        let light = r.kinds[DamageKind::Glass.index()] + r.kinds[DamageKind::RoofCover.index()] + r.kinds[DamageKind::Vegetation.index()];
        assert!(light > 0, "{:?}", r.kinds);
        assert!(walls < 6, "walls {walls}");
        let (ef, ..) = r.rating();
        assert!(ef.unwrap_or(0) <= 2, "{ef:?}");
    }

    #[test]
    fn a_violent_tornado_destroys_the_house_progressively() {
        let mut world = MemoryWorld::flat(64);
        world.house(-4, -3);
        world.tree(12, 6, 6);
        let (records, engine) = run(SpawnShape::Wedge, 95.0, 10.0, &mut world, Vec3::new(-40.0, 64.0, 0.0));
        let r = records.values().next().unwrap();
        assert!(r.blocks_destroyed > 80, "{} {:?}", r.blocks_destroyed, r.kinds);
        assert!(r.kinds[DamageKind::Wall.index()] + r.kinds[DamageKind::Collapse.index()] > 20, "{:?}", r.kinds);
        let (ef, w, di, ..) = r.rating();
        assert!(ef.unwrap() >= 3, "{ef:?} {w} {di}");
        assert!(engine.debris.total_launched > 10);
        // the report renders
        assert!(r.report(60).len() > 8);
    }
}
