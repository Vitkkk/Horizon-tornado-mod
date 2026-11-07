// The damage engine's view of the world: a cache of block columns around tornadoes, read in
// bulk through the host (Horizon's `native_read_columns`), changed locally as damage happens
// and written back in batches (`native_edit_blocks`), so the engine never touches the world
// block by block.

use super::materials::{AIR, Material, classify};
use crate::math::FastMap;
use std::collections::HashMap;

/// Interned block state. 16 bits keep cached columns small; a world has far fewer distinct
/// states than that, and any beyond it read as air.
pub type StateId = u16;

/// Interned block states with their materials.
#[derive(Debug, Default)]
pub struct Palette {
    states: Vec<String>,
    materials: Vec<Material>,
    index: FastMap<String, StateId>,
}

impl Palette {
    pub fn intern(&mut self, state: &str) -> StateId {
        if let Some(&id) = self.index.get(state) {
            return id;
        }
        if self.states.len() >= StateId::MAX as usize {
            return 0;
        }
        let id = self.states.len() as StateId;
        self.states.push(state.to_string());
        self.materials.push(classify(state));
        self.index.insert(state.to_string(), id);
        id
    }
    pub fn state(&self, id: StateId) -> &str {
        self.states.get(id as usize).map_or("minecraft:air", String::as_str)
    }
    pub fn material(&self, id: StateId) -> &Material {
        self.materials.get(id as usize).unwrap_or(&AIR)
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

/// Blocks of one column the engine holds.
#[derive(Clone, Debug)]
pub struct Column {
    pub min_y: i32,
    pub blocks: Vec<StateId>,
    /// Height of the top natural-ground block (terrain under buildings and trees).
    pub ground: i32,
    /// The ground height when first read: scouring never digs far below it.
    pub original_ground: i32,
    /// Simulation time of the read.
    pub read_at: f64,
    /// Simulation time this column's damage was last evaluated.
    pub evaluated_at: f64,
    /// The last tornado whose survey counted this column's structures (0: none).
    pub observed_by: u32,
}

impl Column {
    pub fn max_y(&self) -> i32 {
        self.min_y + self.blocks.len() as i32
    }
    pub fn get(&self, y: i32) -> Option<StateId> {
        let i = y - self.min_y;
        (i >= 0 && (i as usize) < self.blocks.len()).then(|| self.blocks[i as usize])
    }
}

/// What the host answered to a column read: for each column, `below + above` blocks from
/// `base - below` upward (base: the column's motion-blocking-no-leaves height), as indexes
/// into `palette`, one column after another; `None` bases for columns not loaded.
#[derive(Clone, Debug, Default)]
pub struct ColumnData {
    pub palette: Vec<String>,
    pub bases: Vec<Option<i32>>,
    pub blocks: Vec<u16>,
}

/// The host world, as the damage engine needs it.
pub trait WorldIo {
    fn read_columns(&mut self, columns: &[(i32, i32)], below: u32, above: u32) -> Result<ColumnData, String>;
    /// Set these block states at once; answers the number of blocks changed.
    fn edit_blocks(&mut self, edits: &[([i32; 3], String)]) -> Result<u64, String>;
}

/// Blocks read below and above each column's height.
pub const READ_BELOW: u32 = 32;
pub const READ_ABOVE: u32 = 10;

/// Default for the most columns the engine keeps (`ServerSettings::max_cached_columns`).
pub const MAX_COLUMNS: usize = 180_000;

#[derive(Debug, Default)]
pub struct VoxelWindow {
    pub palette: Palette,
    columns: FastMap<(i32, i32), Column>,
    /// Changes not yet written to the world, by column: (y, state).
    pending: FastMap<(i32, i32), Vec<(i32, StateId)>>,
    pending_count: usize,
    air: StateId,
    /// Most columns kept; past it, those farthest from every footprint go first.
    pub cap: usize,
}

impl VoxelWindow {
    pub fn new() -> Self {
        let mut w = VoxelWindow { cap: MAX_COLUMNS, ..Default::default() };
        w.air = w.palette.intern("minecraft:air");
        w
    }

    pub fn air(&self) -> StateId {
        self.air
    }

    pub fn column(&self, x: i32, z: i32) -> Option<&Column> {
        self.columns.get(&(x, z))
    }
    pub fn column_mut(&mut self, x: i32, z: i32) -> Option<&mut Column> {
        self.columns.get_mut(&(x, z))
    }
    pub fn has_column(&self, x: i32, z: i32) -> bool {
        self.columns.contains_key(&(x, z))
    }
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// The block at `pos`; None outside the cached columns. Above a cached column is air.
    pub fn get(&self, pos: [i32; 3]) -> Option<StateId> {
        let c = self.columns.get(&(pos[0], pos[2]))?;
        if pos[1] >= c.max_y() {
            return Some(self.air);
        }
        c.get(pos[1])
    }

    pub fn material(&self, pos: [i32; 3]) -> Option<&Material> {
        self.get(pos).map(|id| self.palette.material(id))
    }

    pub fn state(&self, pos: [i32; 3]) -> Option<&str> {
        self.get(pos).map(|id| self.palette.state(id))
    }

    /// Change a block in the cache and queue the change for the world. False outside the cache.
    pub fn set(&mut self, pos: [i32; 3], state: StateId) -> bool {
        let Some(c) = self.columns.get_mut(&(pos[0], pos[2])) else {
            return false;
        };
        let i = pos[1] - c.min_y;
        if i < 0 || i as usize >= c.blocks.len() {
            return false;
        }
        if c.blocks[i as usize] == state {
            return false;
        }
        c.blocks[i as usize] = state;
        let col = self.pending.entry((pos[0], pos[2])).or_default();
        match col.iter_mut().find(|(y, _)| *y == pos[1]) {
            Some(e) => e.1 = state,
            None => {
                col.push((pos[1], state));
                self.pending_count += 1;
            }
        }
        true
    }

    pub fn set_state(&mut self, pos: [i32; 3], state: &str) -> bool {
        let id = self.palette.intern(state);
        self.set(pos, id)
    }

    pub fn remove(&mut self, pos: [i32; 3]) -> bool {
        let air = self.air;
        self.set(pos, air)
    }

    /// Columns not cached yet among `wanted`.
    pub fn missing(&self, wanted: &[(i32, i32)]) -> Vec<(i32, i32)> {
        wanted.iter().copied().filter(|c| !self.columns.contains_key(c)).collect()
    }

    /// Take in a host read.
    pub fn insert_read(&mut self, columns: &[(i32, i32)], data: &ColumnData, below: u32, above: u32, now: f64) {
        let span = (below + above) as usize;
        let ids: Vec<StateId> = data.palette.iter().map(|s| self.palette.intern(s)).collect();
        for (i, &(x, z)) in columns.iter().enumerate() {
            let Some(Some(base)) = data.bases.get(i) else {
                continue;
            };
            let Some(raw) = data.blocks.get(i * span..(i + 1) * span) else {
                continue;
            };
            let blocks: Vec<StateId> = raw.iter().map(|&b| ids.get(b as usize).copied().unwrap_or(self.air)).collect();
            let min_y = base - below as i32;
            let mut col = Column { min_y, blocks, ground: min_y, original_ground: min_y, read_at: now, evaluated_at: now, observed_by: 0 };
            col.ground = find_ground(&col, &self.palette);
            if let Some(old) = self.columns.get(&(x, z)) {
                col.original_ground = old.original_ground;
                col.evaluated_at = old.evaluated_at;
                col.observed_by = old.observed_by;
            } else {
                col.original_ground = col.ground;
            }
            // keep our own unflushed edits over what the world still says
            if let Some(edits) = self.pending.get(&(x, z)) {
                for &(y, state) in edits {
                    let k = y - min_y;
                    if k >= 0 && (k as usize) < col.blocks.len() {
                        col.blocks[k as usize] = state;
                    }
                }
            }
            self.columns.insert((x, z), col);
        }
    }

    /// Recompute a column's ground after scouring changed it.
    pub fn refresh_ground(&mut self, x: i32, z: i32) {
        if let Some(c) = self.columns.get(&(x, z)) {
            let g = find_ground(c, &self.palette);
            if let Some(c) = self.columns.get_mut(&(x, z)) {
                c.ground = g;
            }
        }
    }

    /// Height of the highest non-air block of a cached column.
    pub fn top(&self, x: i32, z: i32) -> Option<i32> {
        let c = self.columns.get(&(x, z))?;
        (0..c.blocks.len()).rev().find(|&i| !self.palette.material(c.blocks[i]).is_air()).map(|i| c.min_y + i as i32)
    }

    /// Up to `max` queued changes, as states, oldest positions first by coordinate.
    pub fn take_edits(&mut self, max: usize) -> Vec<([i32; 3], String)> {
        let mut cols: Vec<(i32, i32)> = self.pending.keys().copied().collect();
        cols.sort_unstable();
        let mut out = Vec::new();
        for c in cols {
            if out.len() >= max {
                break;
            }
            let Some(mut edits) = self.pending.remove(&c) else { continue };
            edits.sort_unstable_by_key(|e| e.0);
            let room = max - out.len();
            if edits.len() > room {
                let rest = edits.split_off(room);
                self.pending.insert(c, rest);
            }
            self.pending_count -= edits.len();
            for (y, s) in edits {
                out.push(([c.0, y, c.1], self.palette.state(s).to_string()));
            }
        }
        out
    }

    pub fn pending_edits(&self) -> usize {
        self.pending_count
    }

    /// Forget columns outside every circle of `keep` (centre x, z and radius), and columns
    /// read before `stale_before` (they are read again when needed). Columns with unflushed
    /// edits stay. Past `MAX_COLUMNS`, the farthest go too.
    pub fn evict(&mut self, keep: &[(f64, f64, f64)], stale_before: f64) {
        let pending = &self.pending;
        let dist = |x: i32, z: i32| -> f64 {
            keep.iter()
                .map(|&(kx, kz, r)| ((x as f64 - kx).powi(2) + (z as f64 - kz).powi(2)).sqrt() - r)
                .fold(f64::INFINITY, f64::min)
        };
        self.columns.retain(|&(x, z), c| pending.contains_key(&(x, z)) || (dist(x, z) <= 0.0 && c.read_at >= stale_before));
        if self.columns.len() > self.cap {
            let mut by: Vec<(f64, (i32, i32))> = self.columns.keys().map(|&(x, z)| (dist(x, z), (x, z))).collect();
            by.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
            let extra = self.columns.len() - self.cap * 9 / 10;
            for (_, k) in by.into_iter().take(extra) {
                if !self.pending.contains_key(&k) {
                    self.columns.remove(&k);
                }
            }
        }
    }

    /// Whether the cache is full (the engine then reads no more).
    pub fn full(&self) -> bool {
        self.columns.len() >= self.cap
    }

    /// Iterate over cached column coordinates.
    pub fn column_keys(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.columns.keys().copied()
    }
}

/// The top natural-ground block of a column: scanning down, the first terrain block resting on
/// terrain (so a stone floor on dirt is not ground, the dirt is).
fn find_ground(c: &Column, palette: &Palette) -> i32 {
    for i in (1..c.blocks.len()).rev() {
        let m = palette.material(c.blocks[i]);
        if m.terrain && palette.material(c.blocks[i - 1]).terrain {
            return c.min_y + i as i32;
        }
    }
    c.min_y
}

/// An in-memory world for tests and benchmarks: a sparse map of block states over a base
/// terrain of grass on dirt on stone.
pub mod testing {
    use super::*;

    #[derive(Debug, Default)]
    pub struct MemoryWorld {
        pub blocks: HashMap<[i32; 3], String>,
        /// Highest explicitly set block of each column.
        tops: HashMap<(i32, i32), i32>,
        pub ground: i32,
        pub edits: u64,
        pub reads: u64,
    }

    impl MemoryWorld {
        pub fn flat(ground: i32) -> Self {
            MemoryWorld { ground, ..Default::default() }
        }
        pub fn get(&self, p: [i32; 3]) -> String {
            if let Some(s) = self.blocks.get(&p) {
                return s.clone();
            }
            let g = self.ground;
            if p[1] > g {
                "minecraft:air".into()
            } else if p[1] == g {
                "minecraft:grass_block[snowy=false]".into()
            } else if p[1] >= g - 3 {
                "minecraft:dirt".into()
            } else if p[1] > -64 {
                "minecraft:stone".into()
            } else {
                "minecraft:bedrock".into()
            }
        }
        pub fn set(&mut self, p: [i32; 3], s: &str) {
            self.blocks.insert(p, s.to_string());
            let t = self.tops.entry((p[0], p[2])).or_insert(p[1]);
            *t = (*t).max(p[1]);
        }
        /// A box of `state` (inclusive corners).
        pub fn fill(&mut self, a: [i32; 3], b: [i32; 3], state: &str) {
            for x in a[0].min(b[0])..=a[0].max(b[0]) {
                for y in a[1].min(b[1])..=a[1].max(b[1]) {
                    for z in a[2].min(b[2])..=a[2].max(b[2]) {
                        self.set([x, y, z], state);
                    }
                }
            }
        }
        /// A small frame house: plank walls with glass windows, a stair roof, on the ground.
        pub fn house(&mut self, x0: i32, z0: i32) {
            let g = self.ground;
            let (w, d, h) = (7, 6, 4);
            for x in x0..x0 + w {
                for z in z0..z0 + d {
                    self.set([x, g + 1, z], "minecraft:oak_planks"); // floor
                    let edge = x == x0 || x == x0 + w - 1 || z == z0 || z == z0 + d - 1;
                    for y in g + 2..g + 2 + h {
                        if edge {
                            let window = y == g + 3 && (x - x0) % 3 == 1 || y == g + 3 && (z - z0) % 3 == 1;
                            self.set([x, y, z], if window { "minecraft:glass_pane" } else { "minecraft:oak_planks" });
                        }
                    }
                    self.set([x, g + 2 + h, z], "minecraft:oak_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]");
                }
            }
        }
        /// An oak tree: a trunk of `height` logs and a leaf crown.
        pub fn tree(&mut self, x: i32, z: i32, height: i32) {
            let g = self.ground;
            for y in g + 1..=g + height {
                self.set([x, y, z], "minecraft:oak_log[axis=y]");
            }
            for dx in -2i32..=2 {
                for dz in -2i32..=2 {
                    for dy in -1i32..=1 {
                        if dx.abs() + dz.abs() + dy.abs() <= 3 && !(dx == 0 && dz == 0 && dy <= 0) {
                            self.set([x + dx, g + height + dy, z + dz], "minecraft:oak_leaves[distance=1,persistent=false,waterlogged=false]");
                        }
                    }
                }
            }
        }
        fn height(&self, x: i32, z: i32) -> i32 {
            // motion blocking, no leaves: one above the highest solid non-leaf block
            let top = self.tops.get(&(x, z)).copied().unwrap_or(self.ground).max(self.ground);
            for y in (self.ground..=top).rev() {
                let m = classify(&self.get([x, y, z]));
                if !m.is_air() && m.kind != super::super::materials::Kind::Leaves && m.kind != super::super::materials::Kind::Plant {
                    return y + 1;
                }
            }
            self.ground + 1
        }
    }

    impl WorldIo for MemoryWorld {
        fn read_columns(&mut self, columns: &[(i32, i32)], below: u32, above: u32) -> Result<ColumnData, String> {
            self.reads += columns.len() as u64;
            let mut palette: Vec<String> = Vec::new();
            let mut index: HashMap<String, u16> = HashMap::new();
            let mut out = ColumnData::default();
            for &(x, z) in columns {
                let base = self.height(x, z);
                out.bases.push(Some(base));
                for y in base - below as i32..base + above as i32 {
                    let s = self.get([x, y, z]);
                    let id = *index.entry(s.clone()).or_insert_with(|| {
                        palette.push(s.clone());
                        (palette.len() - 1) as u16
                    });
                    out.blocks.push(id);
                }
            }
            out.palette = palette;
            Ok(out)
        }
        fn edit_blocks(&mut self, edits: &[([i32; 3], String)]) -> Result<u64, String> {
            for (p, s) in edits {
                self.set(*p, s);
            }
            self.edits += edits.len() as u64;
            Ok(edits.len() as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MemoryWorld;
    use super::*;

    #[test]
    fn window_reads_edits_and_finds_ground_under_a_house() {
        let mut world = MemoryWorld::flat(64);
        world.house(0, 0);
        let mut w = VoxelWindow::new();
        let cols: Vec<(i32, i32)> = (0..7).flat_map(|x| (0..6).map(move |z| (x, z))).collect();
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 0.0);
        assert_eq!(w.column(2, 2).unwrap().ground, 64);
        assert_eq!(w.state([0, 66, 0]), Some("minecraft:oak_planks"));
        assert!(w.remove([0, 66, 0]));
        assert_eq!(w.material([0, 66, 0]).unwrap().kind, super::super::materials::Kind::Air);
        let edits = w.take_edits(100);
        assert_eq!(edits, vec![([0, 66, 0], "minecraft:air".to_string())]);
        world.edit_blocks(&edits).unwrap();
        assert_eq!(world.get([0, 66, 0]), "minecraft:air");
        // a re-read keeps pending edits and the original ground
        w.set_state([1, 64, 1], "minecraft:dirt");
        let data = world.read_columns(&cols, READ_BELOW, READ_ABOVE).unwrap();
        w.insert_read(&cols, &data, READ_BELOW, READ_ABOVE, 1.0);
        assert_eq!(w.state([1, 64, 1]), Some("minecraft:dirt"));
    }
}
