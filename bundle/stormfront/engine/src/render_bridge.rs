// The hand-over between the game thread, which builds what to draw, and the render thread,
// which draws it in Horizon's `world_drawn` hook (render.rs in the bundle). Only plain data
// crosses: no wgpu types here, so this crate stays engine-free and testable.

use std::sync::Mutex;

/// What a particle is drawn as.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shape {
    /// A soft, irregular puff (cloud, condensation, dust).
    #[default]
    Puff = 0,
    /// A hard-edged tumbling chunk (debris, splinters).
    Chunk = 1,
    /// A streak along its velocity (rain).
    Streak = 2,
    /// A small round pellet (hail).
    Pellet = 3,
    /// A flat flickering leaf.
    Leaf = 4,
    /// A line from `pos` to `pos + vel` (lightning, science overlays); `size` is its width.
    Line = 5,
    /// A tall soft sheet (a distant rain shaft); `size` is its width, height is 4 x size.
    Sheet = 6,
}

/// One drawn particle: 36 bytes, laid out as the shader's instance attributes (pos at 0, size
/// at 12, vel at 16, colour at 28 as unorm8x4, shape at 32).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Instance {
    /// Position relative to the frame's anchor (blocks).
    pub pos: [f32; 3],
    pub size: f32,
    /// Velocity (blocks/s): the shader extrapolates `pos + vel * t` between updates. For
    /// lines, the vector from start to end.
    pub vel: [f32; 3],
    /// RGBA, straight alpha.
    pub color: [u8; 4],
    /// Low byte: `Shape`; next byte: a per-particle seed.
    pub shape: u32,
}

impl Instance {
    pub const SIZE: usize = 36;

    /// Little-endian bytes in the shader's layout.
    pub fn write(&self, out: &mut Vec<u8>) {
        for v in self.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.size.to_le_bytes());
        for v in self.vel {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.color);
        out.extend_from_slice(&self.shape.to_le_bytes());
    }
}

/// One frame's drawing, as the game thread last built it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// World position every instance is relative to (keeps f32 precise far from the origin).
    pub anchor: [f64; 3],
    pub instances: Vec<Instance>,
    /// When the game thread built it (seconds, monotonic): the shader extrapolates from it.
    pub built_at: f64,
    /// Ambient light multiplier for the particles (day/night, storm darkness, flash).
    pub light: f32,
    /// Bumped each time a new frame is published.
    pub version: u64,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);

/// The game thread publishes a frame (replacing the previous one).
pub fn publish(mut frame: Frame) {
    if let Ok(mut f) = FRAME.lock() {
        frame.version = f.as_ref().map_or(1, |p| p.version.wrapping_add(1));
        *f = Some(frame);
    }
}

/// The render thread takes the newest frame if it changed since `seen`.
pub fn latest(seen: u64) -> Option<Frame> {
    let f = FRAME.lock().ok()?;
    f.as_ref().filter(|f| f.version != seen).cloned()
}

/// Nothing to draw (left the world, mod turned off).
pub fn clear() {
    if let Ok(mut f) = FRAME.lock() {
        *f = None;
    }
}

/// Seconds on a monotonic clock shared by both threads.
pub fn now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The particle shader (WGSL), compiled by the render module.
pub const PARTICLE_WGSL: &str = include_str!("../shaders/particles.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_bytes_match_the_declared_size() {
        let mut out = Vec::new();
        Instance::default().write(&mut out);
        assert_eq!(out.len(), Instance::SIZE);
    }

    #[test]
    fn publish_and_take() {
        publish(Frame { instances: vec![Instance::default()], ..Default::default() });
        let f = latest(0).unwrap();
        assert_eq!(f.instances.len(), 1);
        assert!(latest(f.version).is_none());
        clear();
        assert!(latest(0).is_none());
    }

    #[test]
    fn shader_declares_its_entry_points() {
        assert!(PARTICLE_WGSL.contains("fn vs_main"));
        assert!(PARTICLE_WGSL.contains("fn fs_main"));
    }
}
