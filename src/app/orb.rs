//! The plain orb, drawn when Crumb's page can't be shown: Caw for Mac's
//! `OrbCanvas` (the raven-blue sphere) in cairo. A still circle at rest; while
//! listening its edge moves with the microphone level.

use gtk::cairo;
use std::f64::consts::PI;

/// The orb's diameter, and the side of the square it is drawn in: room for
/// the edge's travel at full voice and for the glow (as on the Mac).
pub const DIAMETER: f64 = 48.0;
pub const TRAVEL: f64 = 1.354;

pub fn canvas_side(diameter: f64) -> f64 {
    let glow = diameter * 0.28;
    diameter + (diameter * (TRAVEL - 1.0) / 2.0 + glow * 2.5) * 2.0
}

fn rgb(hex: u32) -> (f64, f64, f64) {
    (((hex >> 16) & 0xff) as f64 / 255.0, ((hex >> 8) & 0xff) as f64 / 255.0, (hex & 0xff) as f64 / 255.0)
}

/// Draws the orb centred in a `w`×`h` area. `time` in seconds (already scaled), `level` 0..1.
pub fn draw(cr: &cairo::Context, w: f64, h: f64, diameter: f64, talking: bool, level: f64, time: f64) {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let base = diameter / 2.0;
    let glow = diameter * 0.28;

    let shape = |cr: &cairo::Context| {
        let points = 120;
        for i in 0..=points {
            let angle = i as f64 / points as f64 * PI * 2.0;
            let mut offset = 0.0;
            if talking {
                let layer1 = (angle * 4.0 + time * 2.0).sin() * base * 0.077;
                let layer2 = (angle * 7.0 - time * 3.0).cos() * base * 0.062;
                let voice = (angle * 5.0 + time * 6.0).sin() * (level * base * 0.215);
                offset = layer1 + layer2 + voice;
            }
            let r = base + offset;
            let (x, y) = (cx + angle.cos() * r, cy + angle.sin() * r);
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.close_path();
    };

    // The glow: cairo has no blurred shadow, so a soft radial fade around the edge.
    let alpha = if talking { 0.85 } else { 0.6 };
    let halo = cairo::RadialGradient::new(cx, cy, base * 0.9, cx, cy, base + glow * 2.2);
    halo.add_color_stop_rgba(0.0, 0.231, 0.510, 0.965, alpha * 0.55);
    halo.add_color_stop_rgba(0.35, 0.231, 0.510, 0.965, alpha * 0.22);
    halo.add_color_stop_rgba(1.0, 0.231, 0.510, 0.965, 0.0);
    let _ = cr.set_source(&halo);
    cr.arc(cx, cy, base + glow * 2.2, 0.0, PI * 2.0);
    let _ = cr.fill();

    // The body: a radial gradient from an off-centre highlight, which is what lights it like a sphere.
    let shift = base * if talking { 0.231 } else { 0.115 };
    let t = time * 0.8;
    let (ix, iy) = (cx + t.cos() * shift, cy + t.sin() * shift);
    let body = cairo::RadialGradient::new(ix, iy, base * 0.077, cx, cy, base * 1.154);
    body.set_extend(cairo::Extend::Pad);
    let stops: &[(f64, u32)] =
        if talking { &[(0.0, 0xffffff), (0.22, 0x7dd3fc), (0.52, 0x38bdf8), (0.82, 0x4f7cf7), (1.0, 0x4f46e5)] } else { &[(0.0, 0xe6f4ff), (0.32, 0x60a5fa), (0.72, 0x3b82f6), (1.0, 0x2563eb)] };
    for &(at, hex) in stops {
        let (r, g, b) = rgb(hex);
        body.add_color_stop_rgb(at, r, g, b);
    }
    shape(cr);
    let _ = cr.set_source(&body);
    let _ = cr.fill();

    // The rim.
    shape(cr);
    cr.set_source_rgba(0.404, 0.925, 0.910, if talking { 0.95 } else { 0.75 });
    cr.set_line_width((base * 0.022).max(1.0));
    let _ = cr.stroke();
}

/// The "working" spinner over the orb or Crumb: a white arc going round.
pub fn draw_spinner(cr: &cairo::Context, w: f64, h: f64, radius: f64, time: f64) {
    let start = time * PI * 2.0 / 1.1;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.set_line_width(2.5);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.arc(w / 2.0, h / 2.0, radius, start, start + PI * 1.4);
    let _ = cr.stroke();
}

#[cfg(test)]
mod tests {
    #[test]
    fn canvas_holds_the_glow() {
        // The Mac's orb window less its 4 pt padding on each side.
        assert!((super::canvas_side(48.0) - 132.0).abs() < 1.0);
        assert!((super::canvas_side(72.0) - 198.0).abs() < 1.0);
    }
}
