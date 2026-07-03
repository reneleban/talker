//! Erzeugt das App-Icon (1024×1024 PNG) für den Bundle-Build (scripts/bundle.sh).
//! macOS-Squircle mit Indigo-Verlauf + weißem Mikrofon-Glyph (SDF wie im Tray).

use std::fs::File;
use std::io::BufWriter;

const SIZE: usize = 1024;
/// macOS-Icon-Grid: Inhalt 824×824 zentriert, Eckradius ~185.
const MARGIN: f32 = 100.0;
const CORNER: f32 = 185.0;

fn main() {
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mask = squircle_alpha(px, py);
            if mask <= 0.0 {
                continue;
            }
            let t = py / SIZE as f32;
            // Verlauf: tiefes Indigo → Nachtblau (passt zur Ribbon-Palette).
            let bg = [
                lerp(38.0, 24.0, t) as u8,
                lerp(36.0, 22.0, t) as u8,
                lerp(84.0, 48.0, t) as u8,
            ];
            let glyph = mic_alpha(px, py);
            let i = (y * SIZE + x) * 4;
            for c in 0..3 {
                rgba[i + c] = lerp(f32::from(bg[c]), 245.0, glyph) as u8;
            }
            rgba[i + 3] = (mask * 255.0) as u8;
        }
    }

    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/icon-1024.png".into());
    let file = File::create(&out).expect("PNG-Datei anlegen");
    let mut encoder = png::Encoder::new(BufWriter::new(file), SIZE as u32, SIZE as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("PNG-Header")
        .write_image_data(&rgba)
        .expect("PNG-Daten");
    println!("Icon geschrieben: {out}");
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Abgerundetes Icon-Rechteck (Signed Distance, 1 px Kantenglättung).
fn squircle_alpha(x: f32, y: f32) -> f32 {
    let half = (SIZE as f32 - 2.0 * MARGIN) / 2.0;
    let (cx, cy) = (SIZE as f32 / 2.0, SIZE as f32 / 2.0);
    let (dx, dy) = (
        (x - cx).abs() - half + CORNER,
        (y - cy).abs() - half + CORNER,
    );
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    (0.5 - (outside + inside - CORNER)).clamp(0.0, 1.0)
}

/// Mikrofon wie in `tray.rs`, skaliert vom 44er-Raster aufs Icon.
fn mic_alpha(x: f32, y: f32) -> f32 {
    const SCALE: f32 = SIZE as f32 / 44.0 * 0.62; // Glyph ~62 % der Kantenlänge
    const OFFSET: f32 = (SIZE as f32 - 44.0 * SCALE) / 2.0;
    let (x, y) = ((x - OFFSET) / SCALE, (y - OFFSET) / SCALE);

    let capsule = sd_segment(x, y, 22.0, 12.0, 22.0, 18.0) - 6.0;
    let holder = {
        let d = ((x - 22.0).powi(2) + (y - 20.0).powi(2)).sqrt();
        if y >= 20.0 {
            (d - 10.0).abs() - 1.4
        } else {
            f32::MAX
        }
    };
    let stem = sd_segment(x, y, 22.0, 30.0, 22.0, 34.5) - 1.4;
    let base = sd_segment(x, y, 15.5, 36.5, 28.5, 36.5) - 1.4;
    let d = capsule.min(holder).min(stem).min(base) * SCALE;
    (0.5 - d).clamp(0.0, 1.0)
}

fn sd_segment(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (px, py) = (x - ax, y - ay);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
    };
    ((px - t * dx).powi(2) + (py - t * dy).powi(2)).sqrt()
}
