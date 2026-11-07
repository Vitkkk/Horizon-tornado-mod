// Turning sweeps into pictures: the usual colour tables and a polar-to-raster PPI, encoded as
// horizontal runs of one colour so a HUD can draw it with a few thousand rectangles.

use super::scanning::{Product, Sweep};
use crate::math::Vec3;

/// ARGB colours; 0 is transparent.
pub type Argb = u32;

const fn argb(r: u8, g: u8, b: u8) -> Argb {
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// NWS-style reflectivity colours in 5 dBZ steps from 5 dBZ.
pub const DBZ_TABLE: [(f32, Argb); 15] = [
    (5.0, argb(0x04, 0xE9, 0xE7)),
    (10.0, argb(0x01, 0x9F, 0xF4)),
    (15.0, argb(0x03, 0x00, 0xF4)),
    (20.0, argb(0x02, 0xFD, 0x02)),
    (25.0, argb(0x01, 0xC5, 0x01)),
    (30.0, argb(0x00, 0x8E, 0x00)),
    (35.0, argb(0xFD, 0xF8, 0x02)),
    (40.0, argb(0xE5, 0xBC, 0x00)),
    (45.0, argb(0xFD, 0x95, 0x00)),
    (50.0, argb(0xFD, 0x00, 0x00)),
    (55.0, argb(0xD4, 0x00, 0x00)),
    (60.0, argb(0xBC, 0x00, 0x00)),
    (65.0, argb(0xF8, 0x00, 0xFD)),
    (70.0, argb(0x98, 0x54, 0xC6)),
    (75.0, argb(0xFF, 0xFF, 0xFF)),
];

pub fn color_dbz(dbz: f32) -> Argb {
    if dbz.is_nan() || dbz < 5.0 {
        return 0;
    }
    DBZ_TABLE.iter().rev().find(|(t, _)| dbz >= *t).map_or(0, |c| c.1)
}

/// Velocity: greens inbound (negative), reds outbound, grey near zero, in m/s up to `max`.
pub fn color_velocity(v: f32, max: f32) -> Argb {
    if v.is_nan() {
        return 0;
    }
    let max = max.max(10.0);
    let s = (v / max).clamp(-1.0, 1.0);
    if s.abs() < 0.04 {
        return argb(0x77, 0x77, 0x77);
    }
    let t = s.abs();
    let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t) as u8;
    if s < 0.0 {
        // pale green to deep green, bright cyan-green at the extreme
        if t < 0.85 { argb(lerp(0x9A, 0x00, t / 0.85), lerp(0xE6, 0x6E, t / 0.85), lerp(0x9A, 0x10, t / 0.85)) } else { argb(0x00, 0xFF, 0xC0) }
    } else if t < 0.85 {
        argb(lerp(0xF0, 0x8A, t / 0.85), lerp(0x9A, 0x00, t / 0.85), lerp(0x9A, 0x00, t / 0.85))
    } else {
        argb(0xFF, 0xE0, 0x40)
    }
}

/// Correlation coefficient: purple/blue low (debris, non-meteorological), green, yellow,
/// orange to dark red near 1, as on common displays.
pub fn color_cc(cc: f32) -> Argb {
    if cc.is_nan() {
        return 0;
    }
    match cc {
        c if c < 0.45 => argb(0x2A, 0x00, 0x50),
        c if c < 0.65 => argb(0x5A, 0x1E, 0x9E),
        c if c < 0.75 => argb(0x3C, 0x5A, 0xE0),
        c if c < 0.82 => argb(0x2E, 0x9E, 0xF0),
        c if c < 0.88 => argb(0x30, 0xC8, 0x60),
        c if c < 0.92 => argb(0x9A, 0xE0, 0x30),
        c if c < 0.95 => argb(0xF0, 0xE0, 0x20),
        c if c < 0.97 => argb(0xF5, 0xA0, 0x10),
        c if c < 0.99 => argb(0xE0, 0x30, 0x10),
        _ => argb(0x90, 0x10, 0x30),
    }
}

/// A square picture of a sweep, `size` pixels across `range` world blocks either side of
/// `center`, north up.
#[derive(Clone, Debug, PartialEq)]
pub struct RadarImage {
    pub size: usize,
    pub pixels: Vec<Argb>,
}

pub fn rasterize(sweep: &Sweep, product: Product, size: usize, center: Vec3, range: f64) -> RadarImage {
    let mut pixels = vec![0; size * size];
    let vmax = if sweep.nyquist > 0.0 { sweep.nyquist as f32 } else { 50.0 };
    for py in 0..size {
        for px in 0..size {
            let dx = ((px as f64 + 0.5) / size as f64 * 2.0 - 1.0) * range;
            let dz = ((py as f64 + 0.5) / size as f64 * 2.0 - 1.0) * range;
            let at = center + Vec3::new(dx, 0.0, dz);
            let Some((a, g)) = sweep.locate(at) else { continue };
            let i = sweep.idx(a, g);
            pixels[py * size + px] = match product {
                Product::Reflectivity => color_dbz(sweep.dbz[i]),
                Product::Velocity => color_velocity(sweep.vel[i], vmax),
                Product::Correlation => {
                    // CC is only meaningful where there is echo worth showing
                    if sweep.dbz[i] >= 10.0 { color_cc(sweep.cc[i]) } else { 0 }
                }
            };
        }
    }
    RadarImage { size, pixels }
}

/// Horizontal runs of one colour: (x, y, length, colour), transparent pixels left out.
pub fn runs(img: &RadarImage) -> Vec<(u16, u16, u16, Argb)> {
    let mut out = Vec::new();
    for y in 0..img.size {
        let row = &img.pixels[y * img.size..(y + 1) * img.size];
        let mut x = 0;
        while x < img.size {
            let c = row[x];
            let start = x;
            while x < img.size && row[x] == c {
                x += 1;
            }
            if c != 0 {
                out.push((start as u16, y as u16, (x - start) as u16, c));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes() {
        assert_eq!(color_dbz(0.0), 0);
        assert_eq!(color_dbz(52.0), DBZ_TABLE[9].1);
        assert_ne!(color_velocity(-30.0, 40.0), color_velocity(30.0, 40.0));
        let green = color_velocity(-30.0, 40.0);
        assert!((green >> 8 & 0xFF) > (green >> 16 & 0xFF), "inbound is green");
        assert_ne!(color_cc(0.5), color_cc(0.99));
    }

    #[test]
    fn runs_merge_equal_pixels() {
        let img = RadarImage { size: 4, pixels: vec![1, 1, 0, 2, 0, 0, 0, 0, 3, 3, 3, 3, 0, 4, 4, 0] };
        assert_eq!(runs(&img), vec![(0, 0, 2, 1), (3, 0, 1, 2), (0, 2, 4, 3), (1, 3, 2, 4)]);
    }
}
