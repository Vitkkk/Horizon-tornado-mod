// Spatial sound cues, played by the client half with Horizon's `native_play_local_sound`, from
// vanilla sound events (no new assets): a wind roar that grows as a tornado approaches, thunder
// delayed by the distance to the strike, breaking glass and wood, falling trees, collapses,
// rain and hail. At most a few cues a tick, nearest first.

use crate::math::Vec3;

#[derive(Clone, Debug, PartialEq)]
pub struct SoundCue {
    pub sound: &'static str,
    pub at: Vec3,
    pub volume: f32,
    pub pitch: f32,
    /// Ticks from now (0..1200).
    pub delay: i32,
}

pub const SPEED_OF_SOUND: f64 = 343.0;

/// A sound heard from `source` by a listener at `ear`: placed a few blocks from the listener in
/// the source's direction, so its loudness is ours to choose rather than vanilla's attenuation.
pub fn directional(sound: &'static str, ear: Vec3, source: Vec3, loudness: f64, pitch: f64, delay: i32) -> Option<SoundCue> {
    if loudness <= 0.02 {
        return None;
    }
    let d = source - ear;
    let dir = if d.length() > 1.0 { d.normalized() } else { Vec3::new(0.0, 0.0, 1.0) };
    let at = ear + dir * 6.0;
    Some(SoundCue { sound, at, volume: loudness.clamp(0.0, 1.0) as f32, pitch: pitch.clamp(0.5, 2.0) as f32, delay: delay.clamp(0, 1200) })
}

/// The tornado roar: louder with intensity, size and nearness; deeper for large vortices.
pub fn roar(ear: Vec3, tornado: Vec3, v_max: f64, r_max: f64) -> Option<SoundCue> {
    let d = (tornado - ear).horizontal_length();
    let audible = 600.0 + 15.0 * v_max + 2.0 * r_max;
    let near = (1.0 - d / audible).max(0.0);
    let loud = near.powf(1.6) * (v_max / 70.0).clamp(0.3, 1.4);
    let pitch = 0.9 - 0.35 * (r_max / 300.0).min(1.0);
    directional("minecraft:item.elytra.flying", ear, tornado, loud, pitch, 0)
}

/// Thunder for a strike at `strike`: the crack if close, the rumble after the sound's travel.
pub fn thunder(ear: Vec3, strike: Vec3) -> Vec<SoundCue> {
    let d = (strike - ear).length();
    let delay = (d / SPEED_OF_SOUND * 20.0) as i32;
    let mut out = Vec::new();
    if d < 200.0 {
        out.extend(directional("minecraft:entity.lightning_bolt.impact", ear, strike, 1.0 - d / 250.0, 1.0, 0));
    }
    let loud = (1.0 - d / 9000.0).max(0.0);
    out.extend(directional("minecraft:entity.lightning_bolt.thunder", ear, strike, loud, 0.8 + 0.2 * (1.0 - loud), delay));
    out
}

/// What breaking `state` sounds like.
pub fn break_sound(state: &str) -> &'static str {
    if state.contains("glass") {
        "minecraft:block.glass.break"
    } else if state.contains("leaves") || state.contains("grass") || state.contains("flower") {
        "minecraft:block.grass.break"
    } else if state.contains("dirt") || state.contains("sand") || state.contains("gravel") {
        "minecraft:block.gravel.break"
    } else if state.contains("brick") || state.contains("stone") || state.contains("concrete") || state.contains("cobble") {
        "minecraft:block.stone.break"
    } else {
        "minecraft:block.wood.break"
    }
}

/// A local event (burst, tree fall, collapse) at `at`.
pub fn event(sound: &'static str, ear: Vec3, at: Vec3, base: f64, pitch: f64) -> Option<SoundCue> {
    let d = (at - ear).length();
    let delay = (d / SPEED_OF_SOUND * 20.0) as i32;
    directional(sound, ear, at, base * (1.0 - d / 400.0).max(0.0), pitch, delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roar_grows_as_the_tornado_approaches() {
        let ear = Vec3::ZERO;
        let far = roar(ear, Vec3::new(1500.0, 0.0, 0.0), 80.0, 100.0).map_or(0.0, |c| c.volume);
        let near = roar(ear, Vec3::new(200.0, 0.0, 0.0), 80.0, 100.0).unwrap().volume;
        assert!(near > far);
        assert!(roar(ear, Vec3::new(9000.0, 0.0, 0.0), 80.0, 100.0).is_none());
    }

    #[test]
    fn thunder_is_delayed_by_distance() {
        let cues = thunder(Vec3::ZERO, Vec3::new(3430.0, 0.0, 0.0));
        let t = cues.iter().find(|c| c.sound.ends_with("thunder")).unwrap();
        assert_eq!(t.delay, 200);
        assert!(thunder(Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0)).iter().any(|c| c.sound.ends_with("impact")));
    }
}
