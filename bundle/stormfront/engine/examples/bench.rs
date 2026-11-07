// CPU benchmark of the three hot paths, on an in-memory world (no Horizon needed):
//   server: weather + damage + debris + sync for a violent wedge crossing a small town
//   client: particle emission/update/instance building per 20 Hz tick, per quality
//   radar:  one full sweep at each quality's resolution
// Run: cargo run --release --example bench
// Numbers are for the machine it runs on; measure on the target phone before trusting them there.

use std::time::Instant;
use stormfront::client::ClientState;
use stormfront::config::{ClientSettings, Quality};
use stormfront::damage::world::testing::MemoryWorld;
use stormfront::damage::world::{ColumnData, WorldIo};

/// Times the host's side (reads and edits) apart from the engine's.
struct Timed<'a> {
    world: &'a mut MemoryWorld,
    ms: f64,
}

impl WorldIo for Timed<'_> {
    fn read_columns(&mut self, columns: &[(i32, i32)], below: u32, above: u32) -> Result<ColumnData, String> {
        let t = Instant::now();
        let r = self.world.read_columns(columns, below, above);
        self.ms += t.elapsed().as_secs_f64() * 1000.0;
        r
    }
    fn edit_blocks(&mut self, edits: &[([i32; 3], String)]) -> Result<u64, String> {
        let t = Instant::now();
        let r = self.world.edit_blocks(edits);
        self.ms += t.elapsed().as_secs_f64() * 1000.0;
        r
    }
}
use stormfront::math::Vec3;
use stormfront::radar::scanning::{RadarSite, compute_sweep};
use stormfront::sim::{OVERWORLD, PlayerView, ServerSim};

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p) as usize]
}

fn main() {
    let mut world = MemoryWorld::flat(64);
    for i in 0..6 {
        for j in 0..4 {
            world.house(-40 + i * 14, -30 + j * 14);
        }
    }
    for k in 0..30 {
        world.tree(-80 + k * 6, 40, 5 + k % 3);
    }
    let player = PlayerView { uuid: 1, name: "bench".into(), pos: Vec3::new(0.0, 65.0, 0.0), dimension: OVERWORLD.into(), permission: 4 };
    let mut sim = ServerSim::new(42);
    sim.command("storm env outbreak", &player);
    sim.command("tornado spawn wedge", &player);
    sim.command("tornado intensity 95", &player);
    sim.command("tornado speed 12", &player);
    let mut client = ClientState::new(ClientSettings::new());
    let mut server_ms = Vec::new();
    let mut client_ms = Vec::new();
    let mut host_ms = Vec::new();
    let mut peak_columns = 0usize;
    let ticks: usize = std::env::var("BENCH_TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(20 * 30);
    for _ in 0..ticks {
        let t0 = Instant::now();
        let mut io = Timed { world: &mut world, ms: 0.0 };
        let out = sim.tick(&mut io, std::slice::from_ref(&player), 6000);
        let host = io.ms;
        let engine_ms = t0.elapsed().as_secs_f64() * 1000.0 - host;
        if engine_ms > 15.0 && std::env::var("BENCH_SPIKES").is_ok() {
            eprintln!("spike {:.1} ms at tick {}: {:?} debris {}", engine_ms, sim.tick, sim.damage.last, sim.damage.debris.pieces.len());
        }
        server_ms.push(engine_ms);
        host_ms.push(host);
        peak_columns = peak_columns.max(sim.damage.window.column_count());
        for m in &out {
            client.on_message(&m.data);
        }
        let t1 = Instant::now();
        client.tick(player.pos, 6000);
        client_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
    }
    let r = sim.records.values().next().or(sim.finished.last());
    println!("server tick (wedge 95 m/s over a town, {} s), engine only:", ticks / 20);
    println!("  median {:.2} ms, p95 {:.2} ms, max {:.2} ms", percentile(&mut server_ms.clone(), 0.5), percentile(&mut server_ms.clone(), 0.95), percentile(&mut server_ms, 1.0));
    println!("  peak cached columns {peak_columns} (~{:.0} MB of block data)", peak_columns as f64 * (42.0 * 2.0 + 80.0) / 1e6);
    println!("  (the in-memory test world's reads/edits took a further median {:.2} ms; Horizon's are native)", percentile(&mut host_ms, 0.5));
    if let Some(r) = r {
        println!("  {} blocks destroyed, {} debris launched, {} column reads", r.blocks_destroyed, r.debris_launched, world.reads);
    }
    println!("client tick (medium): median {:.2} ms, p95 {:.2} ms, {} particles", percentile(&mut client_ms.clone(), 0.5), percentile(&mut client_ms, 0.95), client.particles.len());
    if std::env::var("BENCH_QUICK").is_ok() {
        return;
    }
    for q in [Quality::Low, Quality::Medium, Quality::High] {
        client.command(&format!("sfx quality {}", q.name()));
        let mut ms = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            client.tick(player.pos, 6000);
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("client tick ({}): median {:.2} ms, {} particles", q.name(), percentile(&mut ms, 0.5), client.particles.len());
        let (radials, gates) = q.radar_resolution();
        let site = RadarSite::new(Vec3::new(0.0, 80.0, 6000.0), 4.0);
        let t = Instant::now();
        let s = compute_sweep(&sim.weather, &site, 0.5, radials, gates, q.radar_subsamples(), 7);
        println!("radar sweep ({}x{} x{}): {:.1} ms (worker thread), {} couplets", radials, gates, q.radar_subsamples(), t.elapsed().as_secs_f64() * 1000.0, s.couplets.len());
    }
}
