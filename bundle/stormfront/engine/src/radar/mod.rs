// The Doppler weather radar: it observes the simulated storms, tornadoes and debris through a
// realistic beam (scanning.rs), producing reflectivity (reflectivity.rs), radial velocity
// (velocity.rs) and correlation coefficient (correlation.rs) sweeps, detects rotation and
// debris signatures, and keeps a history for loops. Sweeps are computed on a worker thread from
// a snapshot of the weather, so the game thread only hands over snapshots and reads results.

pub mod correlation;
pub mod display;
pub mod reflectivity;
pub mod scanning;
pub mod velocity;

use crate::storms::system::StormSystem;
use scanning::{RadarSite, Sweep, compute_sweep};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};

/// One sweep to compute.
struct Job {
    system: StormSystem,
    site: RadarSite,
    elevation: f64,
    radials: usize,
    gates: usize,
    subsamples: usize,
    seed: u64,
    tick: u64,
    tilt: usize,
}

/// A radar running its volume coverage pattern.
pub struct Radar {
    pub site: RadarSite,
    /// The latest complete sweep of each tilt of the pattern.
    pub latest: Vec<Option<Sweep>>,
    /// Past lowest-tilt sweeps, oldest first (for loops).
    pub history: VecDeque<Sweep>,
    pub history_len: usize,
    pub radials: usize,
    pub gates: usize,
    pub subsamples: usize,
    /// Ticks between sweeps (the antenna's pace).
    pub sweep_interval: u64,
    next_tilt: usize,
    last_start: u64,
    busy: bool,
    jobs: Option<Sender<Job>>,
    results: Option<Receiver<(usize, Sweep)>>,
    seed: u64,
}

impl Radar {
    pub fn new(site: RadarSite, radials: usize, gates: usize, subsamples: usize) -> Self {
        let tilts = site.vcp.len();
        Radar {
            site,
            latest: vec![None; tilts],
            history: VecDeque::new(),
            history_len: 12,
            radials,
            gates,
            subsamples,
            sweep_interval: 40,
            next_tilt: 0,
            last_start: 0,
            busy: false,
            jobs: None,
            results: None,
            seed: 0x5ADA,
        }
    }

    /// Start the worker thread. Without it, `tick` computes sweeps inline.
    pub fn start_worker(&mut self) -> bool {
        let (job_tx, job_rx) = channel::<Job>();
        let (res_tx, res_rx) = channel::<(usize, Sweep)>();
        let spawned = std::thread::Builder::new().name("stormfront-radar".into()).spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let mut s = compute_sweep(&job.system, &job.site, job.elevation, job.radials, job.gates, job.subsamples, job.seed);
                s.tick = job.tick;
                if res_tx.send((job.tilt, s)).is_err() {
                    break;
                }
            }
        });
        if spawned.is_ok() {
            self.jobs = Some(job_tx);
            self.results = Some(res_rx);
            true
        } else {
            false
        }
    }

    /// Collect finished sweeps and start the next one when it is time.
    pub fn tick(&mut self, system: &StormSystem, game_tick: u64) {
        let done: Vec<(usize, Sweep)> = match &self.results {
            Some(rx) => rx.try_iter().collect(),
            None => Vec::new(),
        };
        for (tilt, sweep) in done {
            self.busy = false;
            self.store(tilt, sweep);
        }
        if self.busy || game_tick < self.last_start + self.sweep_interval {
            return;
        }
        self.last_start = game_tick;
        let tilt = self.next_tilt % self.site.vcp.len().max(1);
        self.next_tilt = (tilt + 1) % self.site.vcp.len().max(1);
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let job = Job {
            system: system.snapshot(),
            site: self.site.clone(),
            elevation: self.site.vcp[tilt],
            radials: self.radials,
            gates: self.gates,
            subsamples: self.subsamples,
            seed: self.seed,
            tick: game_tick,
            tilt,
        };
        match &self.jobs {
            Some(tx) => {
                if tx.send(job).is_ok() {
                    self.busy = true;
                } else {
                    self.jobs = None;
                }
            }
            None => {
                let mut s = compute_sweep(&job.system, &job.site, job.elevation, job.radials, job.gates, job.subsamples, job.seed);
                s.tick = game_tick;
                self.store(tilt, s);
            }
        }
    }

    fn store(&mut self, tilt: usize, sweep: Sweep) {
        if tilt == 0 {
            self.history.push_back(sweep.clone());
            while self.history.len() > self.history_len {
                self.history.pop_front();
            }
        }
        if let Some(slot) = self.latest.get_mut(tilt) {
            *slot = Some(sweep);
        }
    }

    /// The sweep shown for tilt `tilt`, `back` volumes ago for the lowest tilt's loop.
    pub fn sweep(&self, tilt: usize, back: usize) -> Option<&Sweep> {
        if tilt == 0 && back > 0 {
            let n = self.history.len();
            return (back < n).then(|| &self.history[n - 1 - back]);
        }
        self.latest.get(tilt).and_then(|s| s.as_ref())
    }

    /// Change the site (moving the radar clears its data).
    pub fn set_site(&mut self, site: RadarSite) {
        self.latest = vec![None; site.vcp.len()];
        self.history.clear();
        self.site = site;
        self.next_tilt = 0;
    }
}

impl StormSystem {
    /// A copy of the weather for another thread (the radar's worker).
    pub fn snapshot(&self) -> StormSystem {
        let mut s = StormSystem::new(self.env.clone(), self.scale, 0);
        s.storms = self.storms.clone();
        s.tornadoes = self.tornadoes.clone();
        s.ground_y = self.ground_y;
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::Environment;
    use crate::atmosphere::presets::Preset;
    use crate::config::WorldScale;
    use crate::math::Vec3;
    use crate::storms::{StormKind, SupercellType};

    #[test]
    fn worker_produces_sweeps_through_the_pattern() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Classic), WorldScale::default(), 1);
        sys.spawn_storm(StormKind::Supercell(SupercellType::Classic), Vec3::new(2000.0, 64.0, -3000.0), 0.0);
        let mut radar = Radar::new(RadarSite::new(Vec3::new(0.0, 80.0, 0.0), 4.0), 120, 80, 1);
        radar.sweep_interval = 1;
        assert!(radar.start_worker());
        let mut tick = 0;
        let start = std::time::Instant::now();
        while radar.latest.iter().filter(|s| s.is_some()).count() < 3 && start.elapsed().as_secs() < 20 {
            tick += 1;
            radar.tick(&sys, tick);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(radar.latest[0].is_some() && radar.latest[1].is_some());
        let s = radar.sweep(0, 0).unwrap();
        assert!(s.dbz.iter().any(|v| !v.is_nan() && *v > 30.0), "the storm echoes");
    }

    #[test]
    fn inline_mode_without_worker() {
        let sys = StormSystem::new(Environment::from_preset(Preset::Classic), WorldScale::default(), 1);
        let mut radar = Radar::new(RadarSite::new(Vec3::new(0.0, 80.0, 0.0), 4.0), 60, 40, 1);
        radar.sweep_interval = 1;
        radar.tick(&sys, 1);
        assert!(radar.latest[0].is_some());
        assert!(radar.latest[0].as_ref().unwrap().dbz.iter().all(|v| v.is_nan()), "clear air");
    }
}
