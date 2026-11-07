// Stormfront Lite: the Stormfront simulation as a client mod for Horizon builds that run
// WebAssembly `.hmod` mods (Horizon Android 0.7.x) but cannot compile native mods.
//
// What it keeps from the full mod: the atmosphere, the storms and their life cycle, the
// tornado's 3D wind field and condensation funnel, multiple vortices, the Doppler radar.
// What changes: a client mod cannot read blocks or draw in the world, so
//   - damage is applied with vanilla `fill ... replace <material>` commands, material by
//     material, wherever the simulated wind at that height beats the material's failure speed
//     (the same strengths as the full engine) for long enough, sub-block by sub-block with
//     varying construction quality, so it progresses from leaves and glass to walls;
//   - the funnel, debris cloud and wall cloud are vanilla particles sent along the wind;
//   - flying debris are falling blocks thrown with the wind;
//   - rain and thunder are vanilla weather and lightning.
// It runs the player's own commands, so the world needs cheats. Everything here is plain
// Rust; lib.rs connects it to the game.

use std::collections::{HashMap, VecDeque};
use stormfront::atmosphere::Environment;
use stormfront::atmosphere::presets::Preset;
use stormfront::config::WorldScale;
use stormfront::damage::materials::classify;
use stormfront::damage::structures::building_quality;
use stormfront::math::{FastMap, Rng, Vec3, hash01};
use stormfront::radar::display::{DBZ_TABLE, color_cc, color_dbz, color_velocity};
use stormfront::radar::scanning::{Product, RadarSite, Sweep, compute_radials, detect};
use stormfront::storms::StormKind;
use stormfront::storms::system::{StormSystem, WeatherEvent};
use stormfront::tornado::{SpawnShape, Tornado, wind_class};
use stormfront::units::{RHO_SURFACE, TICK, ms_to_kmh};

/// Commands a tick may send (the host takes 256 quiet commands per call).
const COMMANDS_PER_TICK: usize = 240;
const DAMAGE_PER_TICK: usize = 96;
/// Shapes the host draws in a corner panel.
const MAX_SHAPES: usize = 1000;
/// Radar image cells and their size in GUI pixels.
const RADAR_CELLS: usize = 72;
const RADAR_PX: f32 = 2.0;
/// Damage cells (4 x 4 columns) evaluated per tick, while the command queue has room.
const CELLS_PER_TICK: usize = 90;
/// Cell size (blocks).
const CELL: i32 = 4;
/// Farthest a Lite tornado damages (blocks): bounds the work on a phone.
const MAX_FOOTPRINT: f64 = 140.0;

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerIn {
    pub pos: Vec3,
    pub dimension: String,
}

/// A shape for the radar panel, in panel pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Fill { x: f32, y: f32, w: f32, h: f32, argb: u32 },
    Text { x: f32, y: f32, text: String, rgb: u32, shadow: bool },
}

/// What a tick asks the game to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Out {
    pub commands: Vec<String>,
    pub chat: Vec<String>,
    /// Lines for the status panel (None: leave it; Some(empty): clear it).
    pub status: Option<Vec<(String, u32)>>,
    /// The radar panel: (width, height, shapes); Some(None) removes it.
    pub radar: Option<Option<(f32, f32, Vec<Shape>)>>,
    pub shake: Option<(f32, f32)>,
    /// Settings to keep (storage key "settings").
    pub save: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Band {
    Ground,
    Low,
    Canopy,
}

/// A material class the damage acts on with one `fill ... replace` filter each.
struct Class {
    name: &'static str,
    filters: &'static [&'static str],
    /// What the matching blocks become.
    to: &'static str,
    band: Band,
    /// Wind (m/s) at which a typical block of the class lets go.
    v_fail: f64,
    /// Heights above the ground the fill covers.
    range: (i32, i32),
    /// Flying debris thrown when it fails (a falling block state).
    debris: Option<&'static str>,
    structural: bool,
}

/// The wind at which a block of `state` fails in a typical spot: the full engine's
/// strength, connectivity and support, under a typical exposure coefficient.
fn failure_speed(state: &str, support: f64, exposure: f64) -> f64 {
    let m = classify(state);
    let r = m.strength * (1.0 + m.connect * support);
    (2.0 * r / (RHO_SURFACE * exposure)).sqrt()
}

fn classes() -> Vec<Class> {
    vec![
        Class {
            name: "vegetação rasteira",
            filters: &["#minecraft:small_flowers", "minecraft:short_grass", "minecraft:tall_grass"],
            to: "minecraft:air",
            band: Band::Ground,
            v_fail: failure_speed("minecraft:poppy", 0.0, 1.6),
            range: (-1, 4),
            debris: None,
            structural: false,
        },
        Class {
            name: "folhas",
            filters: &["#minecraft:leaves"],
            to: "minecraft:air",
            band: Band::Canopy,
            v_fail: failure_speed("minecraft:oak_leaves", 1.0, 1.3),
            range: (1, 36),
            debris: None,
            structural: false,
        },
        Class {
            name: "vidros",
            filters: &["minecraft:glass_pane", "#minecraft:impermeable"],
            to: "minecraft:air",
            band: Band::Low,
            v_fail: failure_speed("minecraft:glass_pane", 1.0, 1.05),
            range: (0, 30),
            debris: None,
            structural: true,
        },
        Class {
            name: "cobertura do telhado",
            filters: &["#minecraft:wooden_stairs", "#minecraft:wooden_slabs"],
            to: "minecraft:air",
            band: Band::Low,
            v_fail: failure_speed("minecraft:oak_stairs", 0.9, 1.15),
            range: (1, 30),
            debris: Some("minecraft:oak_slab"),
            structural: true,
        },
        Class {
            name: "portas, cercas, lã",
            filters: &["#minecraft:wooden_doors", "#minecraft:wooden_fences", "#minecraft:wool"],
            to: "minecraft:air",
            band: Band::Low,
            v_fail: failure_speed("minecraft:oak_door", 1.5, 1.05),
            range: (0, 30),
            debris: Some("minecraft:oak_slab"),
            structural: true,
        },
        Class {
            name: "grama arrancada",
            filters: &["minecraft:grass_block"],
            to: "minecraft:dirt",
            band: Band::Ground,
            v_fail: 48.0,
            range: (-3, 6),
            debris: None,
            structural: false,
        },
        Class {
            name: "troncos quebrados",
            filters: &["#minecraft:logs"],
            to: "minecraft:air",
            band: Band::Canopy,
            v_fail: 50.0,
            range: (3, 36),
            debris: Some("minecraft:oak_log"),
            structural: false,
        },
        Class {
            name: "paredes de madeira",
            filters: &["#minecraft:planks"],
            to: "minecraft:air",
            band: Band::Low,
            v_fail: failure_speed("minecraft:oak_planks", 1.95, 1.05),
            range: (1, 30),
            debris: Some("minecraft:oak_slab"),
            structural: true,
        },
        Class {
            name: "solo superficial",
            filters: &["minecraft:dirt"],
            to: "minecraft:coarse_dirt",
            band: Band::Ground,
            v_fail: 62.0,
            range: (-2, 4),
            debris: None,
            structural: false,
        },
        Class {
            name: "descascamento",
            filters: &["minecraft:oak_log", "minecraft:birch_log", "minecraft:spruce_log", "minecraft:dark_oak_log"],
            to: "",
            band: Band::Canopy,
            v_fail: 66.0,
            range: (0, 36),
            debris: None,
            structural: false,
        },
        Class {
            name: "alvenaria",
            filters: &["minecraft:bricks", "minecraft:cobblestone", "minecraft:stone_bricks"],
            to: "minecraft:air",
            band: Band::Low,
            v_fail: failure_speed("minecraft:bricks", 1.95, 1.05),
            range: (1, 30),
            debris: Some("minecraft:cobblestone_slab"),
            structural: true,
        },
    ]
}

/// One tornado's path and what was done along it.
#[derive(Clone, Debug, Default)]
struct Track {
    points: Vec<(f64, f64, f64)>,
    length: f64,
    max_width: f64,
    peak: f64,
    fills: HashMap<&'static str, u32>,
}

struct Radar {
    site: RadarSite,
    product: Product,
    tilt: usize,
    sweep: Sweep,
    truth: Vec<f32>,
    next: usize,
    latest: Option<Sweep>,
    range: f64,
    open: bool,
    rng: Rng,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub damage: bool,
    pub lightning: bool,
    pub sound: bool,
    pub shake: bool,
    /// Particle commands per tick (density).
    pub particles: usize,
    pub weather: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { damage: true, lightning: true, sound: true, shake: true, particles: 90, weather: true }
    }
}

pub struct Lite {
    pub sys: StormSystem,
    pub settings: Settings,
    classes: Vec<Class>,
    /// Doses per damage cell and class, and which classes a cell lost already.
    doses: FastMap<(i32, i32, u8), f32>,
    done: FastMap<(i32, i32), u32>,
    queue: VecDeque<String>,
    debris_queue: VecDeque<String>,
    tracks: HashMap<u32, Track>,
    finished: Vec<(u32, Track)>,
    radar: Option<Radar>,
    rng: Rng,
    tick: u64,
    scan_cursor: usize,
    raining: bool,
    player: Option<PlayerIn>,
    warned_cheats: bool,
    status_shown: bool,
}

fn fmt(v: f64) -> String {
    format!("{:.2}", v)
}

impl Lite {
    pub fn new(seed: u64) -> Self {
        Lite {
            sys: StormSystem::new(Environment::from_preset(Preset::Classic), WorldScale::default(), seed),
            settings: Settings::default(),
            classes: classes(),
            doses: FastMap::default(),
            done: FastMap::default(),
            queue: VecDeque::new(),
            debris_queue: VecDeque::new(),
            tracks: HashMap::new(),
            finished: Vec::new(),
            radar: None,
            rng: Rng::new(seed ^ 0x117E),
            tick: 0,
            scan_cursor: 0,
            raining: false,
            player: None,
            warned_cheats: false,
            status_shown: false,
        }
    }

    pub fn settings_string(&self) -> String {
        let s = &self.settings;
        format!("damage={} lightning={} sound={} shake={} particles={} weather={}", s.damage, s.lightning, s.sound, s.shake, s.particles, s.weather)
    }

    pub fn load_settings(&mut self, text: &str) {
        for kv in text.split_whitespace() {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let b = v == "true";
            match k {
                "damage" => self.settings.damage = b,
                "lightning" => self.settings.lightning = b,
                "sound" => self.settings.sound = b,
                "shake" => self.settings.shake = b,
                "weather" => self.settings.weather = b,
                "particles" => {
                    if let Ok(n) = v.parse::<usize>() {
                        self.settings.particles = n.clamp(10, 140);
                    }
                }
                _ => {}
            }
        }
    }

    /// One game tick.
    pub fn tick(&mut self, player: Option<PlayerIn>, day_time: u64) -> Out {
        let _ = day_time;
        self.tick += 1;
        let mut out = Out::default();
        let in_overworld = player.as_ref().is_some_and(|p| p.dimension == "minecraft:overworld");
        self.player = player.clone();
        let Some(p) = player.filter(|_| in_overworld) else {
            if self.status_shown {
                out.status = Some(Vec::new());
                self.status_shown = false;
            }
            return out;
        };
        let active = !self.sys.storms.is_empty() || !self.sys.tornadoes.is_empty();
        if active {
            self.sys.step(TICK);
            self.events(&p, &mut out);
            self.record();
            if self.settings.damage {
                self.damage(&p);
            }
        }
        let mut budget = COMMANDS_PER_TICK;
        // damage first (it is what the simulation decided), then debris, effects, particles
        for _ in 0..DAMAGE_PER_TICK.min(budget) {
            match self.queue.pop_front() {
                Some(c) => out.commands.push(c),
                None => break,
            }
        }
        // at most ten flying pieces a second: each lands and stays as a block
        if self.tick % 2 == 0 {
            if let Some(c) = self.debris_queue.pop_front() {
                out.commands.push(c);
            }
        }
        budget = budget.saturating_sub(out.commands.len());
        if active {
            self.weather_commands(&p, &mut out, &mut budget);
            self.sounds(&p, &mut out, &mut budget);
            self.particles(&p, &mut out, budget.min(self.settings.particles));
            self.shake(&p, &mut out);
        }
        self.radar_step(&p, &mut out);
        if self.tick % 10 == 0 {
            let lines = self.status(&p);
            if !lines.is_empty() || self.status_shown {
                self.status_shown = !lines.is_empty();
                out.status = Some(lines);
            }
        }
        if self.queue.len() > 4000 {
            // a burst bigger than the game takes: drop the oldest rather than lag forever
            let n = self.queue.len() - 4000;
            self.queue.drain(..n);
        }
        out
    }

    fn events(&mut self, p: &PlayerIn, out: &mut Out) {
        for e in std::mem::take(&mut self.sys.events) {
            match e {
                WeatherEvent::Lightning { pos, .. } => {
                    let d = (pos - p.pos).horizontal_length();
                    if self.settings.lightning && d < 300.0 {
                        let y = p.pos.y.floor();
                        out.commands.push(format!("summon minecraft:lightning_bolt {} {} {}", fmt(pos.x), fmt(y), fmt(pos.z)));
                    } else if self.settings.sound && d < 4000.0 {
                        let loud = (1.0 - d / 4000.0).max(0.1);
                        out.commands.push(format!("playsound minecraft:entity.lightning_bolt.thunder weather @s ~ ~ ~ {} 0.8", fmt(loud)));
                    }
                }
                WeatherEvent::TornadoFormed { id, pos, .. } => {
                    out.chat.push(format!("[Stormfront] Tornado #{id} formou-se a {:.0} m", (pos - p.pos).horizontal_length()));
                }
                WeatherEvent::TornadoDissipated { id, .. } => {
                    if let Some(t) = self.tracks.remove(&id) {
                        out.chat.push(format!("[Stormfront] Tornado #{id} dissipou: {:.1} km, pico {:.0} m/s. /tornado survey {id}", t.length / 1000.0, t.peak));
                        self.finished.push((id, t));
                        if self.finished.len() > 16 {
                            self.finished.remove(0);
                        }
                    }
                }
                WeatherEvent::StormFormed { .. } => {}
            }
        }
    }

    fn record(&mut self) {
        if self.tick % 20 != 0 {
            return;
        }
        for t in &self.sys.tornadoes {
            let tr = self.tracks.entry(t.id).or_default();
            if let Some(last) = tr.points.last() {
                tr.length += ((t.pos.x - last.0).powi(2) + (t.pos.z - last.1).powi(2)).sqrt();
            }
            tr.points.push((t.pos.x, t.pos.z, t.params.v_max));
            tr.max_width = tr.max_width.max(2.0 * t.radius_of_wind(29.0));
            tr.peak = tr.peak.max(t.peak_wind());
        }
    }

    /// Evaluate damage cells around each tornado and queue the fills their winds call for.
    /// The queue drains at the rate the game takes commands; cells are evaluated only while
    /// it has room, so the damage advances as fast as it can be applied and never piles up.
    fn damage(&mut self, p: &PlayerIn) {
        if self.queue.len() > DAMAGE_PER_TICK {
            return;
        }
        let g = self.sys.ground_y;
        let feet: Vec<(u32, Vec3, f64)> = self
            .sys
            .tornadoes
            .iter()
            .filter(|t| (t.pos - p.pos).horizontal_length() < 600.0)
            .map(|t| (t.id, t.pos, (t.radius_of_wind(30.0) + 6.0).min(MAX_FOOTPRINT)))
            .collect();
        if feet.is_empty() {
            return;
        }
        let mut todo: Vec<(i32, i32, u32)> = Vec::new();
        for (id, c, r) in &feet {
            let n = (*r / CELL as f64).ceil() as i32;
            let (cx, cz) = ((c.x / CELL as f64).floor() as i32, (c.z / CELL as f64).floor() as i32);
            for dx in -n..=n {
                for dz in -n..=n {
                    if ((dx * dx + dz * dz) * CELL * CELL) as f64 <= r * r {
                        todo.push((cx + dx, cz + dz, *id));
                    }
                }
            }
        }
        let total = todo.len();
        if total == 0 {
            return;
        }
        let take = CELLS_PER_TICK.min(total);
        // each cell is visited every total/take ticks: integrate that long
        let dt = TICK * (total as f64 / take as f64).max(1.0);
        for k in 0..take {
            let (cx, cz, id) = todo[(self.scan_cursor + k) % total];
            self.cell(cx, cz, id, g, dt);
            if self.queue.len() > DAMAGE_PER_TICK * 3 {
                self.scan_cursor = (self.scan_cursor + k + 1) % total;
                return;
            }
        }
        self.scan_cursor = (self.scan_cursor + take) % total.max(1);
        if self.tick % 200 == 0 {
            let keep: Vec<(f64, f64)> = feet.iter().map(|f| (f.1.x, f.1.z)).collect();
            let near = |cx: i32, cz: i32| keep.iter().any(|&(x, z)| ((cx * CELL) as f64 - x).powi(2) + ((cz * CELL) as f64 - z).powi(2) < 400.0 * 400.0);
            self.doses.retain(|k, _| near(k.0, k.1));
            self.done.retain(|k, _| near(k.0, k.1));
        }
    }

    fn cell(&mut self, cx: i32, cz: i32, tornado: u32, g: f64, dt: f64) {
        let (x, z) = (cx * CELL, cz * CELL);
        let mid = CELL as f64 / 2.0;
        let at = |h: f64| Vec3::new(x as f64 + mid, g + h, z as f64 + mid);
        let ground = self.sys.wind_at(at(1.5)).horizontal_length();
        if ground < 22.0 {
            return;
        }
        let low = self.sys.wind_at(at(6.0)).horizontal_length();
        let canopy = self.sys.wind_at(at(14.0)).horizontal_length();
        let done = self.done.get(&(cx, cz)).copied().unwrap_or(0);
        let mut newly = 0u32;
        for (i, c) in self.classes.iter().enumerate() {
            if done & (1 << i) != 0 {
                continue;
            }
            let v = match c.band {
                Band::Ground => ground,
                Band::Low => low,
                Band::Canopy => canopy,
            };
            // construction quality and local defects
            let mut q = 0.8 + 0.45 * hash01(cx, i as i32, cz, 0x11E);
            if c.structural {
                q *= building_quality(x, z);
            }
            let threshold = c.v_fail * q.sqrt();
            if v < threshold * 0.9 {
                continue;
            }
            let dose = self.doses.entry((cx, cz, i as u8)).or_insert(0.0);
            *dose += (dt * (v - threshold * 0.9).max(0.0).powi(2) / 60.0) as f32;
            if *dose < 1.0 {
                continue;
            }
            newly |= 1 << i;
            let y0 = (g as i32 + c.range.0).max(-64);
            let y1 = (g as i32 + c.range.1).min(319);
            for f in c.filters {
                let to = if c.to.is_empty() { f.replacen("minecraft:", "minecraft:stripped_", 1) } else { c.to.to_string() };
                self.queue.push_back(format!("fill {x} {y0} {z} {} {y1} {} {to} replace {f}", x + CELL - 1, z + CELL - 1));
            }
            let tr = self.tracks.entry(tornado).or_default();
            *tr.fills.entry(c.name).or_insert(0) += 1;
            if let Some(d) = c.debris {
                if self.debris_queue.len() < 8 && self.rng.chance(0.15) {
                    let w = self.sys.wind_at(at(6.0));
                    let m = |v: f64| (v / 20.0).clamp(-3.5, 3.5);
                    self.debris_queue.push_back(format!(
                        "summon minecraft:falling_block {} {} {} {{BlockState:{{Name:\"{d}\"}},Time:1,DropItem:0b,Motion:[{}d,{}d,{}d]}}",
                        x,
                        fmt(g + 3.0),
                        z,
                        fmt(m(w.x)),
                        fmt(m(w.y.max(8.0) + 6.0)),
                        fmt(m(w.z))
                    ));
                }
            }
        }
        if newly != 0 {
            self.done.insert((cx, cz), done | newly);
        }
    }

    /// Vanilla rain and thunder while a storm's core is overhead.
    fn weather_commands(&mut self, p: &PlayerIn, out: &mut Out, budget: &mut usize) {
        if !self.settings.weather || self.tick % 40 != 0 || *budget == 0 {
            return;
        }
        let rain: f64 = self.sys.storms.iter().map(|s| s.hydrometeors(p.pos, 500.0).rain).sum();
        let wet = rain > 0.3;
        if wet != self.raining {
            self.raining = wet;
            out.commands.push(if wet { "weather thunder 6000".into() } else { "weather clear 6000".into() });
            *budget -= 1;
        }
    }

    fn sounds(&mut self, p: &PlayerIn, out: &mut Out, budget: &mut usize) {
        if !self.settings.sound || self.tick % 40 != 0 || *budget < 2 {
            return;
        }
        let Some(t) = self.sys.tornadoes.iter().min_by(|a, b| (a.pos - p.pos).horizontal_length().total_cmp(&(b.pos - p.pos).horizontal_length())) else {
            return;
        };
        if let Some(c) = stormfront::audio::roar(p.pos, t.pos, t.params.v_max, t.params.r_max) {
            let d = c.at - p.pos;
            out.commands.push(format!("playsound {} ambient @s ~{} ~{} ~{} {} {}", c.sound, fmt(d.x), fmt(d.y), fmt(d.z), fmt(c.volume as f64), fmt(c.pitch as f64)));
            *budget -= 1;
        }
    }

    /// The funnel, the suction vortices, the debris cloud and the wall cloud, as particles sent
    /// along the simulated wind.
    fn particles(&mut self, p: &PlayerIn, out: &mut Out, budget: usize) {
        let mut left = budget;
        let mut push = |out: &mut Out, s: String| {
            if left > 0 {
                out.commands.push(s);
                left -= 1;
            }
        };
        let tornadoes: Vec<&Tornado> = self.sys.tornadoes.iter().filter(|t| (t.pos - p.pos).horizontal_length() < 900.0).collect();
        for t in &tornadoes {
            let levels = &t.funnel.levels;
            let lit: Vec<usize> = (0..levels.len()).filter(|&k| levels[k].radius > 0.3).collect();
            if lit.is_empty() {
                continue;
            }
            // a share of the budget per tornado, spread over the condensed levels
            let per = (budget * 6 / 10 / tornadoes.len().max(1)).max(4);
            for n in 0..per {
                let k = lit[(n * 7 + self.tick as usize) % lit.len()];
                let l = levels[k];
                let ang = self.rng.range(0.0, std::f64::consts::TAU);
                let rr = l.radius * self.rng.range(0.6, 1.0);
                let pos = t.pos + l.offset + Vec3::new(ang.cos() * rr, l.height, -ang.sin() * rr);
                let v = t.wind_at(pos) / 20.0;
                let kind = if rr > 8.0 { "minecraft:campfire_cosy_smoke" } else { "minecraft:large_smoke" };
                push(out, format!("particle {kind} {} {} {} {} {} {} 1 0 force", fmt(pos.x), fmt(pos.y), fmt(pos.z), fmt(v.x), fmt(v.y), fmt(v.z)));
            }
            for sv in t.multivortex.vortices.iter().filter(|v| v.envelope() > 0.3) {
                let base = t.pos + sv.offset();
                push(out, format!("particle minecraft:cloud {} {} {} {} 6 {} 0.05 12 force", fmt(base.x), fmt(base.y + 6.0), fmt(base.z), fmt(sv.core * 0.3), fmt(sv.core * 0.3)));
            }
            // the debris cloud where the ground wind is damaging
            let r = t.radius_of_wind(30.0).min(120.0);
            if r > 2.0 {
                let load = (t.debris_load / 1000.0).min(4.0);
                let count = (18.0 + 6.0 * load) as u32;
                push(out, format!("particle minecraft:dust{{color:[0.42,0.34,0.26],scale:4.0}} {} {} {} {} 2 {} 0 {count} force", fmt(t.pos.x), fmt(t.pos.y + 2.0), fmt(t.pos.z), fmt(r * 0.45), fmt(r * 0.45)));
                push(out, format!("particle minecraft:poof {} {} {} {} 1 {} 0.2 10 force", fmt(t.pos.x), fmt(t.pos.y + 1.0), fmt(t.pos.z), fmt(r * 0.3), fmt(r * 0.3)));
            }
        }
        // the storm's base and wall cloud near the player
        if self.tick % 2 == 0 {
            for s in self.sys.storms.iter().filter(|s| (s.meso_center() - p.pos).horizontal_length() < 1500.0 && s.updraft > 5.0) {
                let m = s.meso_center();
                let base = self.sys.ground_y + s.cloud_base * (1.0 - 0.3 * s.meso.strength);
                let w = (s.meso.radius * 0.35).min(60.0);
                push(out, format!("particle minecraft:dust{{color:[0.22,0.22,0.25],scale:4.0}} {} {} {} {} 3 {} 0 30 force", fmt(m.x), fmt(base), fmt(m.z), fmt(w), fmt(w)));
            }
        }
    }

    fn shake(&self, p: &PlayerIn, out: &mut Out) {
        if !self.settings.shake || self.tick % 10 != 0 {
            return;
        }
        let s = self
            .sys
            .tornadoes
            .iter()
            .map(|t| {
                let d = (t.pos - p.pos).horizontal_length();
                let r = t.radius_of_wind(29.0).max(30.0);
                (1.0 - d / (4.0 * r)).max(0.0) * (t.params.v_max / 90.0).min(1.0)
            })
            .fold(0.0, f64::max);
        if s > 0.03 {
            out.shake = Some(((s * 0.5) as f32, 0.6));
        }
    }

    fn status(&self, p: &PlayerIn) -> Vec<(String, u32)> {
        let mut lines = Vec::new();
        if let Some(t) = self.sys.tornadoes.iter().min_by(|a, b| (a.pos - p.pos).horizontal_length().total_cmp(&(b.pos - p.pos).horizontal_length())) {
            let d = t.pos - p.pos;
            let dist = d.horizontal_length();
            if dist < 3000.0 {
                let deg = d.x.atan2(-d.z).to_degrees().rem_euclid(360.0);
                let dir = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][((deg + 22.5) / 45.0) as usize % 8];
                lines.push((format!("AVISO DE TORNADO: {:.0} m a {dir}", dist), 0xFF5050));
                lines.push((
                    format!("#{} {} [{}] {:.0} m/s ({:.0} km/h) T{}", t.id, t.morphology().name(), t.stage.name(), t.params.v_max, ms_to_kmh(t.params.v_max), wind_class(t.peak_wind())),
                    0xFFFFFF,
                ));
            }
        }
        lines
    }

    // ---- radar --------------------------------------------------------------------------

    fn radar_step(&mut self, p: &PlayerIn, out: &mut Out) {
        let Some(r) = &mut self.radar else { return };
        if !r.open {
            return;
        }
        // a few radials a tick: a full sweep every few seconds, inside the tick's budget
        let to = (r.next + 6).min(r.sweep.radials);
        compute_radials(&self.sys, &r.site, &mut r.sweep, &mut r.truth, r.next, to, 1, &mut r.rng);
        r.next = to;
        if r.next >= r.sweep.radials {
            detect(&mut r.sweep, &r.truth);
            r.sweep.tick = self.tick;
            let elev = r.site.vcp[r.tilt.min(r.site.vcp.len() - 1)];
            let fresh = Sweep::new(&r.site, elev, r.sweep.radials, r.sweep.gates);
            r.latest = Some(std::mem::replace(&mut r.sweep, fresh));
            r.truth = vec![f32::NAN; r.sweep.radials * r.sweep.gates];
            r.next = 0;
        }
        if self.tick % 10 == 0 || r.next == 0 {
            out.radar = Some(Some(radar_panel(r, p.pos, self.tick)));
        }
    }

    fn open_radar(&mut self, at: Vec3) {
        let mut site = RadarSite::new(at + Vec3::new(0.0, 15.0, 0.0), 1.0 / self.sys.scale.storm_length);
        site.max_range = 20_000.0;
        let elev = site.vcp[0];
        let (radials, gates) = (180, 100);
        self.radar = Some(Radar {
            sweep: Sweep::new(&site, elev, radials, gates),
            truth: vec![f32::NAN; radials * gates],
            site,
            product: Product::Reflectivity,
            tilt: 0,
            next: 0,
            latest: None,
            range: 10_000.0,
            open: true,
            rng: Rng::new(self.tick ^ 0x7ADA),
        });
    }

    // ---- commands -----------------------------------------------------------------------

    /// `/name args`. The reply's first line is the feedback, the rest go to chat.
    pub fn command(&mut self, name: &str, args: &str, out: &mut Out) -> Result<Vec<String>, String> {
        let p = self.player.clone().ok_or("Entre num mundo primeiro")?;
        if self.sys.storms.is_empty() && self.sys.tornadoes.is_empty() {
            self.sys.ground_y = p.pos.y.floor();
        }
        let mut words = args.split_whitespace();
        let a = words.next();
        let b = words.next();
        let num = |s: Option<&str>| s.and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
        let on = |s: Option<&str>| matches!(s.map(|v| v.to_ascii_lowercase()).as_deref(), Some("on" | "true" | "sim" | "1"));
        let mut lines = Vec::new();
        let cheats = "(precisa de cheats: o mod usa comandos vanilla como você)";
        match name {
            "tornado" => match a {
                Some("spawn") => {
                    let shape = match b {
                        None => SpawnShape::Cone,
                        Some(s) => SpawnShape::parse(s).ok_or_else(|| format!("formas: {}", SpawnShape::NAMES))?,
                    };
                    let at = p.pos + Vec3::new(-120.0, 0.0, 0.0);
                    let id = self.sys.spawn_tornado(shape, Vec3::new(at.x, self.sys.ground_y, at.z));
                    lines.push(format!("Tornado #{id} ({shape:?}) a 120 m a oeste."));
                    if !self.warned_cheats {
                        lines.push(format!("Stormfront Lite {cheats}"));
                        self.warned_cheats = true;
                    }
                }
                Some("intensity") => {
                    let v = num(b).filter(|v| (10.0..=140.0).contains(v)).ok_or("use: /tornado intensity <10..140 m/s>")?;
                    let t = self.sys.nearest_tornado(p.pos).ok_or("Nenhum tornado ativo")?;
                    t.overrides.v_max = Some(v);
                    t.target_v_max = v;
                    lines.push(format!("Tornado #{}: {v:.0} m/s ({:.0} km/h)", t.id, ms_to_kmh(v)));
                }
                Some("speed") => {
                    let v = num(b).filter(|v| (0.0..=40.0).contains(v)).ok_or("use: /tornado speed <0..40 m/s>")?;
                    let t = self.sys.nearest_tornado(p.pos).ok_or("Nenhum tornado ativo")?;
                    t.overrides.speed = Some(v);
                    lines.push(format!("Tornado #{}: translação {v:.1} m/s", t.id));
                }
                Some("multivortex") => {
                    let t = self.sys.nearest_tornado(p.pos).ok_or("Nenhum tornado ativo")?;
                    let v = on(b);
                    t.overrides.multivortex = Some(v);
                    t.multivortex.enabled = v;
                    lines.push(format!("Tornado #{}: múltiplos vórtices {}", t.id, if v { "ligados" } else { "desligados" }));
                }
                Some("damage") => {
                    self.settings.damage = on(b);
                    out.save = Some(self.settings_string());
                    lines.push(format!("Danos {}", if self.settings.damage { "ligados" } else { "desligados" }));
                }
                Some("dissipate") => {
                    let t = self.sys.nearest_tornado(p.pos).ok_or("Nenhum tornado ativo")?;
                    t.overrides = Default::default();
                    t.life = t.age;
                    lines.push(format!("Tornado #{} entrando em dissipação", t.id));
                }
                Some("list") => {
                    for t in &self.sys.tornadoes {
                        lines.push(format!(
                            "#{} {} [{}] a {:.0} m: Vmax {:.0} m/s, Rmax {:.0} m, ventos >= 29 m/s até {:.0} m",
                            t.id,
                            t.morphology().name(),
                            t.stage.name(),
                            (t.pos - p.pos).horizontal_length(),
                            t.params.v_max,
                            t.params.r_max,
                            t.radius_of_wind(29.0)
                        ));
                    }
                    if lines.is_empty() {
                        lines.push("Nenhum tornado ativo".into());
                    }
                }
                Some("survey") => {
                    let id = num(b).map(|v| v as u32);
                    let found = match id {
                        Some(id) => self.tracks.get(&id).map(|t| (id, t.clone())).or_else(|| self.finished.iter().find(|f| f.0 == id).cloned()),
                        None => self.tracks.iter().next().map(|(i, t)| (*i, t.clone())).or_else(|| self.finished.last().cloned()),
                    };
                    let (id, t) = found.ok_or("Nenhum tornado registrado")?;
                    lines.push(format!("Levantamento do tornado #{id}: trajeto {:.0} m, largura máx (>= 29 m/s) {:.0} m, vento de pico {:.0} m/s (T{})", t.length, t.max_width, t.peak, wind_class(t.peak)));
                    let mut fills: Vec<_> = t.fills.iter().collect();
                    fills.sort_by(|a, b| b.1.cmp(a.1));
                    for (k, v) in fills.iter().take(6) {
                        lines.push(format!("  {k}: {v} trechos de 2x2 afetados"));
                    }
                    lines.push("A versão Lite não lê blocos: a classificação EF por danos existe só na versão PC.".into());
                }
                _ => return Err("use: /tornado spawn <forma> | intensity | speed | multivortex | damage | dissipate | list | survey".into()),
            },
            "storm" => match a {
                Some("env") => {
                    let pr = b.and_then(Preset::parse).ok_or("presets: classic, outbreak, hp, lp, qlcs, pulse, microburst, marginal")?;
                    self.sys.set_preset(pr);
                    lines.extend(self.sys.env.summary_lines());
                }
                Some("debug") => {
                    lines.extend(self.sys.env.summary_lines());
                    for s in &self.sys.storms {
                        lines.push(format!("#{} {} [{}] a {:.0} m, meso {:.2}", s.id, s.kind.name(), s.stage.name(), (s.pos - p.pos).horizontal_length(), s.meso.strength));
                    }
                }
                Some("clear") => {
                    self.sys.clear();
                    self.queue.clear();
                    lines.push("Tempestades e tornados removidos.".into());
                    if self.raining {
                        out.commands.push("weather clear 6000".into());
                        self.raining = false;
                    }
                }
                Some("tornado") => {
                    let s = self.sys.storms.iter_mut().find(|s| s.kind.is_supercell() || s.kind == StormKind::Qlcs).ok_or("Nenhuma supercélula ativa")?;
                    s.force_tornado = true;
                    s.meso.strength = s.meso.strength.max(0.8);
                    lines.push(format!("Tempestade #{} vai produzir um tornado.", s.id));
                }
                Some(k) => {
                    let (kind, micro) = if k == "microburst" { (StormKind::Single, true) } else { (StormKind::parse(k).ok_or_else(|| format!("tipos: {}", StormKind::NAMES))?, false) };
                    let dist = num(b).unwrap_or(if micro { 0.0 } else { 3000.0 });
                    let id = self.sys.spawn_storm(kind, p.pos, dist);
                    if micro {
                        if let Some(s) = self.sys.storms.iter_mut().find(|s| s.id == id) {
                            s.age = s.life * 0.86;
                            s.pos = p.pos + Vec3::new(250.0, 0.0, 0.0);
                        }
                    }
                    let s = self.sys.storms.iter().find(|s| s.id == id).unwrap();
                    lines.push(format!("Tempestade #{id}: {} a {:.0} m, {:.1} m/s", s.kind.name(), (s.pos - p.pos).horizontal_length(), s.motion.horizontal_length()));
                    if !self.warned_cheats {
                        lines.push(format!("Stormfront Lite {cheats}"));
                        self.warned_cheats = true;
                    }
                }
                None => return Err("use: /storm <supercell|hp|lp|multicell|single|qlcs|microburst> | env | debug | clear | tornado".into()),
            },
            "radar" => match a {
                None | Some("open") => {
                    if self.radar.is_none() {
                        self.open_radar(p.pos);
                    }
                    if let Some(r) = &mut self.radar {
                        r.open = true;
                    }
                    lines.push("Radar aberto: /radar product reflectivity|velocity|cc, /radar range <km>, /radar here".into());
                }
                Some("close") => {
                    if let Some(r) = &mut self.radar {
                        r.open = false;
                    }
                    out.radar = Some(None);
                    lines.push("Radar fechado".into());
                }
                Some("product") | Some("reflectivity") | Some("velocity") | Some("cc") | Some("ref") | Some("vel") => {
                    let which = if a == Some("product") { b } else { a };
                    let prod = which.and_then(Product::parse).ok_or("use: /radar product reflectivity|velocity|cc")?;
                    if self.radar.is_none() {
                        self.open_radar(p.pos);
                    }
                    if let Some(r) = &mut self.radar {
                        r.product = prod;
                        r.open = true;
                    }
                    lines.push(format!("Produto {}", prod.short()));
                }
                Some("range") => {
                    let km = num(b).filter(|v| (1.0..=20.0).contains(v)).ok_or("use: /radar range <1..20 km>")?;
                    if let Some(r) = &mut self.radar {
                        r.range = km * 1000.0;
                    }
                    lines.push(format!("Alcance {km:.0} km"));
                }
                Some("tilt") => {
                    let i = num(b).ok_or("use: /radar tilt <0..8>")? as usize;
                    if let Some(r) = &mut self.radar {
                        r.tilt = i.min(r.site.vcp.len() - 1);
                        lines.push(format!("Elevação {:.1}° (na próxima varredura)", r.site.vcp[r.tilt]));
                    }
                }
                Some("here") => {
                    self.open_radar(p.pos);
                    lines.push("Radar instalado aqui".into());
                }
                _ => return Err("use: /radar open|close|product <ref|vel|cc>|range <km>|tilt <n>|here".into()),
            },
            "sfx" => {
                match a {
                    Some("particles") => {
                        self.settings.particles = match b {
                            Some("low") => 30,
                            Some("high") => 130,
                            _ => 80,
                        };
                    }
                    Some("lightning") => self.settings.lightning = on(b),
                    Some("sound") => self.settings.sound = on(b),
                    Some("shake") => self.settings.shake = on(b),
                    Some("weather") => self.settings.weather = on(b),
                    _ => return Err("use: /sfx particles low|medium|high | lightning on|off | sound on|off | shake on|off | weather on|off".into()),
                }
                out.save = Some(self.settings_string());
                lines.push(format!("Ajustes: {}", self.settings_string()));
            }
            _ => return Err("comando desconhecido".into()),
        }
        Ok(lines)
    }
}

fn radar_panel(r: &Radar, player: Vec3, tick: u64) -> (f32, f32, Vec<Shape>) {
    let n = RADAR_CELLS;
    let px = RADAR_PX;
    let img_w = n as f32 * px;
    let (w, h) = (img_w + 40.0, img_w + 34.0);
    let mut shapes = vec![Shape::Fill { x: 0.0, y: 0.0, w, h, argb: 0xC010_1418 }];
    let sweep = r.latest.as_ref();
    let elev = sweep.map_or(r.site.vcp[r.tilt.min(r.site.vcp.len() - 1)], |s| s.elevation);
    shapes.push(Shape::Text { x: 3.0, y: 3.0, text: format!("RADAR {} {:.1}°", r.product.short(), elev), rgb: 0xE8E8E8, shadow: true });
    let (ix, iy) = (3.0, 13.0);
    shapes.push(Shape::Fill { x: ix, y: iy, w: img_w, h: img_w, argb: 0xFF05_0807 });
    match sweep {
        None => shapes.push(Shape::Text { x: ix + 4.0, y: iy + img_w / 2.0, text: format!("varrendo {}%", r.next * 100 / r.sweep.radials.max(1)), rgb: 0xA0A0A0, shadow: false }),
        Some(s) => {
            let img = stormfront::radar::display::rasterize(s, r.product, n, r.site.pos, r.range);
            for (x, y, len, c) in stormfront::radar::display::runs(&img) {
                if shapes.len() >= MAX_SHAPES - 40 {
                    break;
                }
                shapes.push(Shape::Fill { x: ix + x as f32 * px, y: iy + y as f32 * px, w: len as f32 * px, h: px, argb: c });
            }
            let to_px = |p: Vec3| -> Option<(f32, f32)> {
                let d = p - r.site.pos;
                let u = (d.x / r.range * 0.5 + 0.5) * img_w as f64;
                let v = (d.z / r.range * 0.5 + 0.5) * img_w as f64;
                (u >= 0.0 && v >= 0.0 && u < img_w as f64 && v < img_w as f64).then_some((ix + u as f32, iy + v as f32))
            };
            for c in &s.couplets {
                if let Some((x, y)) = to_px(c.pos) {
                    let col = if c.tvs { 0xFFFF_2020 } else { 0xFFFF_E040 };
                    shapes.push(Shape::Fill { x: x - 3.0, y: y - 3.0, w: 7.0, h: 1.0, argb: col });
                    shapes.push(Shape::Fill { x: x - 3.0, y: y + 3.0, w: 7.0, h: 1.0, argb: col });
                    shapes.push(Shape::Fill { x: x - 3.0, y: y - 3.0, w: 1.0, h: 7.0, argb: col });
                    shapes.push(Shape::Fill { x: x + 3.0, y: y - 3.0, w: 1.0, h: 7.0, argb: col });
                }
            }
            for t in &s.tds {
                if let Some((x, y)) = to_px(*t) {
                    shapes.push(Shape::Fill { x: x - 2.0, y: y - 2.0, w: 5.0, h: 5.0, argb: 0xFFC0_40FF });
                }
            }
            if let Some((x, y)) = to_px(player) {
                shapes.push(Shape::Fill { x: x - 1.0, y: y - 1.0, w: 3.0, h: 3.0, argb: 0xFF30_A0FF });
            }
            let line = match s.couplets.first() {
                Some(c) => format!(
                    "{} {:.1} km Vrot {:.0} m/s",
                    if s.tds.iter().any(|t| (*t - c.pos).horizontal_length() < 800.0) {
                        "TDS"
                    } else if c.tvs {
                        "TVS"
                    } else {
                        "MESO"
                    },
                    (c.pos - r.site.pos).horizontal_length() / 1000.0,
                    c.v_rot
                ),
                None => format!("sem rotação | {:.0} km | {}s", r.range / 1000.0, tick.saturating_sub(s.tick) / 20),
            };
            shapes.push(Shape::Text { x: 3.0, y: iy + img_w + 3.0, text: line, rgb: 0xE8E8E8, shadow: true });
            // legend
            let lx = ix + img_w + 3.0;
            let steps = 16;
            let cell = img_w / steps as f32;
            for k in 0..steps {
                let f = 1.0 - (k as f32 + 0.5) / steps as f32;
                let c = match r.product {
                    Product::Reflectivity => color_dbz(5.0 + 70.0 * f),
                    Product::Velocity => color_velocity(-40.0 + 80.0 * f, 40.0),
                    Product::Correlation => color_cc(0.4 + 0.6 * f),
                };
                shapes.push(Shape::Fill { x: lx, y: iy + k as f32 * cell, w: 5.0, h: cell.ceil(), argb: c });
            }
            let (top, bottom) = match r.product {
                Product::Reflectivity => ("75", "5 dBZ"),
                Product::Velocity => ("+40", "-40 m/s"),
                Product::Correlation => ("1.0", "0.4 CC"),
            };
            shapes.push(Shape::Text { x: lx + 7.0, y: iy, text: top.into(), rgb: 0xA0A0A0, shadow: false });
            shapes.push(Shape::Text { x: lx + 7.0, y: iy + img_w - 8.0, text: bottom.into(), rgb: 0xA0A0A0, shadow: false });
            let _ = DBZ_TABLE;
        }
    }
    (w, h, shapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> Option<PlayerIn> {
        Some(PlayerIn { pos: Vec3::new(0.0, 65.0, 0.0), dimension: "minecraft:overworld".into() })
    }

    #[test]
    fn failure_speeds_follow_the_full_engines_ordering() {
        let c = classes();
        let v = |n: &str| c.iter().find(|c| c.name == n).unwrap().v_fail;
        assert!(v("folhas") < v("vidros"));
        assert!(v("vidros") < v("cobertura do telhado"));
        assert!(v("cobertura do telhado") < v("paredes de madeira"));
        assert!(v("paredes de madeira") < v("alvenaria"));
    }

    #[test]
    fn a_tornado_produces_damage_particles_and_stays_within_the_hosts_limits() {
        let mut lite = Lite::new(1);
        lite.tick(player(), 6000);
        let mut out = Out::default();
        lite.command("storm", "env outbreak", &mut out).unwrap();
        let lines = lite.command("tornado", "spawn wedge", &mut out).unwrap();
        assert!(lines[0].contains("Tornado #1"));
        lite.command("tornado", "intensity 90", &mut out).unwrap();
        lite.command("radar", "open", &mut out).unwrap();
        let (mut fills, mut parts, mut debris, mut radar) = (0, 0, 0, false);
        for _ in 0..(20 * 40) {
            let out = lite.tick(player(), 6000);
            assert!(out.commands.len() <= 256, "{}", out.commands.len());
            assert!(out.chat.len() <= 8);
            for c in &out.commands {
                assert!(c.len() <= 256, "{c}");
                if c.starts_with("fill ") {
                    fills += 1;
                    assert!(c.contains(" replace "));
                }
                if c.starts_with("particle ") {
                    parts += 1;
                }
                if c.contains("falling_block") {
                    debris += 1;
                }
            }
            if let Some(Some((_, _, shapes))) = &out.radar {
                assert!(shapes.len() <= 1024);
                radar = true;
            }
        }
        assert!(fills > 30, "{fills}");
        assert!(parts > 1000, "{parts}");
        assert!(debris > 0);
        assert!(radar);
        let mut o = Out::default();
        let survey = lite.command("tornado", "survey", &mut o).unwrap();
        assert!(survey[0].contains("Levantamento"));
    }

    #[test]
    fn weak_winds_do_not_reach_walls() {
        let mut lite = Lite::new(2);
        lite.tick(player(), 0);
        let mut out = Out::default();
        lite.command("tornado", "spawn cone", &mut out).unwrap();
        lite.command("tornado", "intensity 30", &mut out).unwrap();
        lite.command("tornado", "speed 0", &mut out).unwrap();
        // start it at that intensity (a spawned cone otherwise ramps down from its own)
        for t in &mut lite.sys.tornadoes {
            t.params.v_max = 30.0;
            t.velocity = Vec3::ZERO;
        }
        let mut walls = 0;
        for _ in 0..(20 * 30) {
            for c in lite.tick(player(), 0).commands {
                if c.contains("#minecraft:planks") || c.contains("minecraft:bricks") {
                    walls += 1;
                }
            }
        }
        assert_eq!(walls, 0);
    }

    #[test]
    fn settings_round_trip() {
        let mut a = Lite::new(3);
        a.settings.lightning = false;
        a.settings.particles = 30;
        let mut b = Lite::new(4);
        b.load_settings(&a.settings_string());
        assert_eq!(a.settings, b.settings);
    }
}
