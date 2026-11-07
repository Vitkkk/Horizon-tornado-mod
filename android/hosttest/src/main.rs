// A stand-in for Horizon Android's mod host (crates/horizon-mods/src/runtime.rs in that
// port): the same wasmtime version, component model, fuel per call and memory limit, the same
// per-call caps on commands, chat and shapes. It loads the packed component, plays a session
// (a player in the Overworld, the commands a player would type, a minute of ticks) and checks
// that nothing traps, runs out of fuel or exceeds a cap, and reports what the mod asked for.
//
//   stormfront-lite-hosttest <mod.wasm>

use std::process::ExitCode;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

mod bindings {
    wasmtime::component::bindgen!({ path: "../stormfront-lite/wit", world: "horizon-mod" });
}
mod render_bindings {
    wasmtime::component::bindgen!({ path: "../stormfront-lite/wit", world: "render-host" });
}
use bindings::HorizonMod;
use bindings::horizon::mods::{chat, command, game, hud, log, storage};
use render_bindings::RenderHost;
use render_bindings::horizon::mods::{input, render};

// Horizon Android's limits (runtime.rs)
const MEMORY: usize = 64 << 20;
const FUEL_INIT: u64 = 2_000_000_000;
const FUEL_TICK: u64 = 50_000_000;
const FUEL_EVENT: u64 = 100_000_000;
const CHAT_PER_CALL: usize = 8;
const QUIET_PER_CALL: usize = 256;
const SHAPES: usize = 1024;
const LINE_CHARS: usize = 256;

#[derive(Default)]
struct Calls {
    quiet: Vec<String>,
    chat: Vec<String>,
    shapes: usize,
    corner: bool,
    shakes: usize,
    panel: Vec<String>,
}

struct State {
    limits: StoreLimits,
    calls: Calls,
    storage: std::collections::HashMap<String, Vec<u8>>,
    player: Option<game::Player>,
    tick: u64,
    problems: Vec<String>,
}

impl log::Host for State {
    fn write(&mut self, _level: log::Level, message: String) {
        eprintln!("  [mod log] {message}");
    }
}
impl game::Host for State {
    fn get_player(&mut self) -> Option<game::Player> {
        self.player.clone()
    }
    fn get_clock(&mut self) -> Option<game::Clock> {
        Some(game::Clock { game_time: self.tick, day_time: 6000 + self.tick })
    }
    fn raycast(&mut self, _distance: f32) -> Option<game::Hit> {
        None
    }
}
impl chat::Host for State {
    fn show(&mut self, text: String) {
        if self.calls.chat.len() >= CHAT_PER_CALL {
            self.problems.push("more than 8 chat lines in one call".into());
        }
        self.calls.chat.push(text);
    }
}
impl hud::Host for State {
    fn set_panel(&mut self, _corner: hud::Corner, lines: Vec<hud::Line>) {
        self.calls.panel = lines.into_iter().map(|l| l.text).collect();
    }
    fn clear(&mut self) {
        self.calls.panel.clear();
    }
}
impl storage::Host for State {
    fn get(&mut self, key: String) -> Option<Vec<u8>> {
        self.storage.get(&key).cloned()
    }
    fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        self.storage.insert(key, value);
        Ok(())
    }
    fn delete(&mut self, key: String) {
        self.storage.remove(&key);
    }
}
impl command::Host for State {
    fn run(&mut self, line: String) {
        self.calls.quiet.push(line);
    }
    fn run_quiet(&mut self, lines: Vec<String>) {
        for l in lines {
            if l.chars().count() > LINE_CHARS {
                self.problems.push(format!("command longer than {LINE_CHARS}: {l}"));
            }
            self.calls.quiet.push(l);
        }
        if self.calls.quiet.len() > QUIET_PER_CALL {
            self.problems.push(format!("{} quiet commands in one call", self.calls.quiet.len()));
        }
    }
}
impl render::Host for State {
    fn set_hud(&mut self, shapes: Vec<render::Shape>) {
        self.calls.shapes = self.calls.shapes.max(shapes.len());
    }
    fn gui_size(&mut self) -> (f32, f32) {
        (427.0, 240.0)
    }
    fn shake(&mut self, strength: f32, seconds: f32) {
        if !(0.0..=1.0).contains(&strength) || !(0.0..=10.0).contains(&seconds) {
            self.problems.push(format!("shake out of range {strength} {seconds}"));
        }
        self.calls.shakes += 1;
    }
    fn set_corner(&mut self, _corner: render::Corner, width: f32, height: f32, shapes: Vec<render::Shape>) {
        if shapes.len() > SHAPES {
            self.problems.push(format!("{} shapes", shapes.len()));
        }
        if !(0.0..=1024.0).contains(&width) || !(0.0..=1024.0).contains(&height) {
            self.problems.push(format!("corner {width}x{height}"));
        }
        self.calls.shapes = self.calls.shapes.max(shapes.len());
        self.calls.corner = true;
    }
    fn clear_corner(&mut self) {
        self.calls.corner = false;
    }
    fn text_width(&mut self, text: String) -> f32 {
        6.0 * text.chars().count() as f32
    }
    fn frame_stats(&mut self) -> render::FrameRate {
        render::FrameRate { fps: 60, low: 50, min: 40 }
    }
    fn set_zoom(&mut self, _zoom: render::Zoom) {}
    fn set_minimap(&mut self, _minimap: Option<render::Minimap>) {}
    fn set_far_terrain(&mut self, _far: Option<render::FarTerrain>) {}
}
impl input::Host for State {
    fn claim_items(&mut self, _ids: Vec<String>) {}
    fn register_key(&mut self, _name: String, _label: String, _key: String) -> Result<(), String> {
        Ok(())
    }
    fn held(&mut self) -> Option<input::HeldItem> {
        None
    }
    fn poll(&mut self) -> Vec<input::Event> {
        Vec::new()
    }
    fn claim_wheel(&mut self, _on: bool) {}
    fn mapping(&mut self, _name: String) -> Option<input::MappingState> {
        None
    }
    fn clicks_per_second(&mut self) -> (u32, u32) {
        (0, 0)
    }
}

fn main() -> ExitCode {
    let path = std::env::args().nth(1).expect("path to mod.wasm");
    let bytes = std::fs::read(&path).expect("read component");
    let mut config = Config::new();
    config.wasm_component_model(true).consume_fuel(true);
    let engine = Engine::new(&config).unwrap();
    let mut linker: Linker<State> = Linker::new(&engine);
    HorizonMod::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s).unwrap();
    RenderHost::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s).unwrap();
    let component = Component::new(&engine, &bytes).expect("a component");
    let state = State {
        limits: StoreLimitsBuilder::new().memory_size(MEMORY).instances(16).build(),
        calls: Calls::default(),
        storage: Default::default(),
        player: None,
        tick: 0,
        problems: Vec::new(),
    };
    let mut store = Store::new(&engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_fuel(FUEL_INIT).unwrap();
    let instance = linker.instantiate(&mut store, &component).expect("instantiate");
    let hooks = HorizonMod::new(&mut store, &instance).expect("events export");
    let events = hooks.horizon_mods_events();
    let reg = events.call_init(&mut store).expect("init trapped").expect("init refused");
    println!("init: commands {:?}, tick {}, fuel used {}", reg.commands, reg.tick, FUEL_INIT - store.get_fuel().unwrap());

    store.data_mut().player = Some(game::Player { x: 0.5, y: 65.0, z: 0.5, yaw: 0.0, pitch: 0.0, health: 20.0, food: 20, dimension: "minecraft:overworld".into() });
    let mut max_tick_fuel = 0u64;
    let mut total_commands = 0usize;
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    let mut sample: Vec<String> = Vec::new();
    let script: Vec<(u64, &str, &str)> = vec![
        (5, "storm", "env outbreak"),
        (10, "radar", "open"),
        (15, "tornado", "spawn wedge"),
        (20, "tornado", "intensity 90"),
        (25, "tornado", "speed 10"),
        (400, "radar", "product velocity"),
        (800, "tornado", "multivortex true"),
        (1100, "tornado", "survey"),
        (1150, "sfx", "particles low"),
    ];
    for tick in 1..=1200u64 {
        store.data_mut().tick = tick;
        for (at, name, args) in &script {
            if *at == tick {
                store.set_fuel(FUEL_EVENT).unwrap();
                store.data_mut().calls = Calls::default();
                let r = events.call_command(&mut store, name, args).expect("command trapped");
                let used = FUEL_EVENT - store.get_fuel().unwrap();
                println!("/{name} {args} -> {r:?} (fuel {used})");
                for c in &store.data().calls.chat {
                    println!("    chat: {c}");
                }
            }
        }
        store.set_fuel(FUEL_TICK).unwrap();
        store.data_mut().calls = Calls::default();
        if let Err(e) = events.call_tick(&mut store, tick) {
            println!("TICK {tick} TRAPPED: {e:?}");
            return ExitCode::FAILURE;
        }
        let used = FUEL_TICK - store.get_fuel().unwrap();
        max_tick_fuel = max_tick_fuel.max(used);
        let calls = std::mem::take(&mut store.data_mut().calls);
        total_commands += calls.quiet.len();
        for c in &calls.quiet {
            let k = c.split_whitespace().next().unwrap_or("").to_string();
            *kinds.entry(k).or_default() += 1;
            if sample.len() < 12 && !sample.iter().any(|s| s.split_whitespace().next() == c.split_whitespace().next()) {
                sample.push(c.clone());
            }
        }
        if tick % 200 == 0 {
            println!("tick {tick}: {} commands this tick, shapes {}, panel {:?}, fuel {used}", calls.quiet.len(), calls.shapes, calls.panel.first());
        }
    }
    println!("max fuel in a tick: {max_tick_fuel} of {FUEL_TICK} ({:.0}%)", max_tick_fuel as f64 * 100.0 / FUEL_TICK as f64);
    println!("commands over 60 s: {total_commands}: {kinds:?}");
    for s in &sample {
        println!("  e.g. {s}");
    }
    println!("settings stored: {:?}", store.data().storage.get("settings").map(|v| String::from_utf8_lossy(v).to_string()));
    let problems = &store.data().problems;
    if problems.is_empty() {
        println!("OK: no trap, within fuel, commands, chat and shape limits");
        ExitCode::SUCCESS
    } else {
        for p in problems.iter().take(10) {
            println!("PROBLEM: {p}");
        }
        ExitCode::FAILURE
    }
}
