// HUD drawing as a list of plain operations (filled rectangles and text in GUI pixels), which
// the glue replays with Horizon's `native_fill` and `native_text`.

use crate::radar::display::{DBZ_TABLE, color_cc, color_velocity, rasterize, runs};
use crate::radar::scanning::{Product, Sweep, beam_height};
use crate::math::Vec3;
use crate::units::ms_to_kmh;

#[derive(Clone, Debug, PartialEq)]
pub enum HudOp {
    Fill { rect: [f32; 4], argb: u32 },
    Text { at: [f32; 2], text: String, rgb: u32, shadow: bool },
}

pub const BACKGROUND: u32 = 0xC010_1418;
pub const INK: u32 = 0xE8E8E8;
pub const DIM: u32 = 0xA0A0A0;
pub const WARN: u32 = 0xFF5050;

/// How the radar panel is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadarView {
    pub product: Product,
    pub tilt: usize,
    /// Display half-width (world blocks).
    pub range: f64,
    pub expanded: bool,
    pub back: usize,
}

impl Default for RadarView {
    fn default() -> Self {
        RadarView { product: Product::Reflectivity, tilt: 0, range: 12_000.0, expanded: false, back: 0 }
    }
}

impl RadarView {
    pub fn image_size(&self) -> usize {
        if self.expanded { 200 } else { 120 }
    }
    /// Panel width and height (GUI px).
    pub fn panel_size(&self) -> (f32, f32) {
        let n = self.image_size() as f32;
        (n + 46.0, n + 58.0)
    }
}

fn compass(v: Vec3) -> &'static str {
    let deg = v.x.atan2(-v.z).to_degrees().rem_euclid(360.0);
    const NAMES: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    NAMES[((deg + 22.5) / 45.0) as usize % 8]
}

/// The radar panel at (x, y). `sweep` is the one shown (None while the first scans).
pub fn radar_panel(x: f32, y: f32, view: &RadarView, sweep: Option<&Sweep>, band: &str, now_tick: u64, player: Vec3, site: Vec3, scale: f64) -> Vec<HudOp> {
    let mut ops = Vec::new();
    let n = view.image_size();
    let (w, h) = view.panel_size();
    ops.push(HudOp::Fill { rect: [x, y, w, h], argb: BACKGROUND });
    let ix = x + 4.0;
    let iy = y + 14.0;
    let elev = sweep.map_or(0.0, |s| s.elevation);
    let age = sweep.map_or(0, |s| now_tick.saturating_sub(s.tick) / 20);
    let title = format!("RADAR {band} {} {:.1}°{}", view.product.short(), elev, if view.back > 0 { format!(" -{}", view.back) } else { String::new() });
    ops.push(HudOp::Text { at: [ix, y + 3.0], text: title, rgb: INK, shadow: true });
    ops.push(HudOp::Fill { rect: [ix, iy, n as f32, n as f32], argb: 0xFF05_0807 });
    let Some(sweep) = sweep else {
        ops.push(HudOp::Text { at: [ix + 4.0, iy + n as f32 / 2.0 - 4.0], text: "varrendo...".into(), rgb: DIM, shadow: false });
        return ops;
    };
    // range rings every quarter of the display
    for k in 1..=2 {
        let r = n as f32 / 2.0 * k as f32 / 2.0;
        for a in 0..48 {
            let ang = a as f32 / 48.0 * std::f32::consts::TAU;
            ops.push(HudOp::Fill { rect: [ix + n as f32 / 2.0 + r * ang.cos(), iy + n as f32 / 2.0 + r * ang.sin(), 1.0, 1.0], argb: 0x60FF_FFFF });
        }
    }
    let img = rasterize(sweep, view.product, n, site, view.range);
    for (rx, ry, len, c) in runs(&img) {
        ops.push(HudOp::Fill { rect: [ix + rx as f32, iy + ry as f32, len as f32, 1.0], argb: c });
    }
    let to_px = |p: Vec3| -> Option<(f32, f32)> {
        let d = p - site;
        let u = (d.x / view.range * 0.5 + 0.5) * n as f64;
        let v = (d.z / view.range * 0.5 + 0.5) * n as f64;
        (u >= 0.0 && v >= 0.0 && u < n as f64 && v < n as f64).then_some((ix + u as f32, iy + v as f32))
    };
    // the site and the player
    ops.push(HudOp::Fill { rect: [ix + n as f32 / 2.0 - 1.0, iy + n as f32 / 2.0 - 1.0, 3.0, 3.0], argb: 0xFFFF_FFFF });
    if let Some((px, py)) = to_px(player) {
        ops.push(HudOp::Fill { rect: [px - 1.0, py - 1.0, 3.0, 3.0], argb: 0xFF30_A0FF });
    }
    // detections
    for c in &sweep.couplets {
        if let Some((px, py)) = to_px(c.pos) {
            let col = if c.tvs { 0xFFFF_2020 } else { 0xFFFF_E040 };
            let s = if c.tvs { 5.0 } else { 7.0 };
            for (dx, dy, ww, hh) in [(-s, -s, 2.0 * s, 1.0), (-s, s, 2.0 * s, 1.0), (-s, -s, 1.0, 2.0 * s), (s, -s, 1.0, 2.0 * s + 1.0)] {
                ops.push(HudOp::Fill { rect: [px + dx, py + dy, ww, hh], argb: col });
            }
        }
    }
    for t in &sweep.tds {
        if let Some((px, py)) = to_px(*t) {
            ops.push(HudOp::Fill { rect: [px - 2.0, py - 2.0, 5.0, 5.0], argb: 0xFFC0_40FF });
        }
    }
    // legend
    let lx = ix + n as f32 + 4.0;
    legend(&mut ops, lx, iy, n as f32, view.product, sweep.nyquist);
    // footer: the strongest detection and the beam height at the display edge
    let fy = iy + n as f32 + 3.0;
    let best = sweep.couplets.first();
    let line = match best {
        Some(c) => {
            let d = c.pos - site;
            let strength = match c.v_rot {
                v if v >= 30.0 => "forte",
                v if v >= 20.0 => "moderada",
                _ => "fraca",
            };
            let kind = if sweep.tds.iter().any(|t| (*t - c.pos).horizontal_length() < 800.0) {
                "TDS"
            } else if c.tvs {
                "TVS"
            } else {
                "MESO"
            };
            format!("{kind} {:.1} km {} Vrot {:.0} m/s ({strength})", d.horizontal_length() / 1000.0, compass(d), c.v_rot)
        }
        None => "sem rotação detectada".to_string(),
    };
    ops.push(HudOp::Text { at: [ix, fy], text: line, rgb: if best.is_some_and(|c| c.tvs) { WARN } else { INK }, shadow: true });
    let edge_h = beam_height(view.range * scale, elev.to_radians());
    ops.push(HudOp::Text {
        at: [ix, fy + 10.0],
        text: format!("{:.0} km  feixe {:.1} km na borda  {age}s", view.range / 1000.0, edge_h / 1000.0),
        rgb: DIM,
        shadow: true,
    });
    ops
}

fn legend(ops: &mut Vec<HudOp>, x: f32, y: f32, h: f32, product: Product, nyquist: f64) {
    let steps = 24;
    let cell = h / steps as f32;
    let (lo, hi, unit): (f32, f32, &str) = match product {
        Product::Reflectivity => (5.0, 75.0, "dBZ"),
        Product::Velocity => {
            let v = if nyquist > 0.0 { nyquist as f32 } else { 50.0 };
            (-v, v, "m/s")
        }
        Product::Correlation => (0.4, 1.0, "CC"),
    };
    for k in 0..steps {
        let v = hi - (hi - lo) * (k as f32 + 0.5) / steps as f32;
        let c = match product {
            Product::Reflectivity => DBZ_TABLE.iter().rev().find(|(t, _)| v >= *t).map_or(0, |c| c.1),
            Product::Velocity => color_velocity(v, hi),
            Product::Correlation => color_cc(v),
        };
        ops.push(HudOp::Fill { rect: [x, y + k as f32 * cell, 6.0, cell.ceil()], argb: c });
    }
    let label = |v: f32| -> String {
        match product {
            Product::Reflectivity => format!("{v:.0}"),
            Product::Velocity => format!("{v:+.0}"),
            Product::Correlation => format!("{v:.2}"),
        }
    };
    ops.push(HudOp::Text { at: [x + 8.0, y], text: label(hi), rgb: DIM, shadow: false });
    ops.push(HudOp::Text { at: [x + 8.0, y + h / 2.0 - 4.0], text: label((hi + lo) / 2.0), rgb: DIM, shadow: false });
    ops.push(HudOp::Text { at: [x + 8.0, y + h - 8.0], text: label(lo), rgb: DIM, shadow: false });
    ops.push(HudOp::Text { at: [x + 8.0, y + h / 4.0], text: unit.to_string(), rgb: INK, shadow: false });
    if product == Product::Velocity {
        ops.push(HudOp::Text { at: [x + 8.0, y + 3.0 * h / 4.0], text: format!("{:.0}km/h", ms_to_kmh(hi as f64)), rgb: DIM, shadow: false });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radar::scanning::{RadarSite, Sweep};

    #[test]
    fn panel_draws_background_and_status_without_data() {
        let ops = radar_panel(0.0, 0.0, &RadarView::default(), None, "S", 0, Vec3::ZERO, Vec3::ZERO, 4.0);
        assert!(matches!(ops[0], HudOp::Fill { .. }));
        assert!(ops.iter().any(|o| matches!(o, HudOp::Text { text, .. } if text.contains("varrendo"))));
    }

    #[test]
    fn panel_draws_a_sweep() {
        let site = RadarSite::new(Vec3::ZERO, 4.0);
        let mut sweep = Sweep::new(&site, 0.5, 36, 10);
        for v in sweep.dbz.iter_mut() {
            *v = 45.0;
        }
        let ops = radar_panel(10.0, 10.0, &RadarView { range: 40_000.0, ..Default::default() }, Some(&sweep), "S", 100, Vec3::ZERO, Vec3::ZERO, 4.0);
        assert!(ops.len() > 100);
    }
}
