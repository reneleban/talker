//! Aufnahme-Indikator: natives, transparentes NSWindow mit Core-Animation-Wellen.
//!
//! Warum nativ statt egui-Viewport: egui kann Kind-Viewports nicht transparent
//! rendern (egui#3632). Look: Leuchtspur-Linien — jede Welle zieht einen über
//! die Zeit ausglühenden Trail hinter sich her (Ghost-Layer mit Alters-Opacity),
//! additiv gemischt (CILinearDodge) und weich geblurrt (CIGaussianBlur), dazu
//! ein Glanzpunkt am Wellenkamm. Fenster existiert einmal (orderFront/orderOut);
//! borderless NSWindows können keinen Fokus annehmen → Injection-Ziel bleibt fokussiert.

use std::cell::Cell;
use std::collections::VecDeque;
use std::ptr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_core_image::CIFilter;
use objc2_foundation::NSString;
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, CATransaction, kCALineCapRound};

use crate::config::{Config, OverlayPosition};
use crate::indicator::{Indicator, Phase, WAVE_LAYERS, wave_samples};

const WIN_H: f64 = 64.0;
const EDGE_MARGIN: f64 = 48.0;
const WAVE_POINTS: usize = 72;
/// NSStatusWindowLevel — über normalen Fenstern und Vollbild-Overlays.
const WINDOW_LEVEL: isize = 25;
/// Maximale Leuchtspur-Länge — Layer werden einmalig angelegt, die konfigurierte
/// Länge (config.overlay_trail_len) blendet nur ein/aus.
const MAX_TRAIL: usize = 12;
/// Anzahl Wellen-Linien (== WAVE_LAYERS.len(), Farben aus der Config).
const N_RIBBONS: usize = 4;

/// Optik-Parameter aus der Config, pro Tick gelesen und bei Änderung angewendet.
#[derive(Clone, PartialEq)]
struct Style {
    trail_len: usize,
    decay: f32,
    colors: Vec<[u8; 3]>,
}

pub struct Overlay {
    window: Retained<NSWindow>,
    group: Retained<CALayer>,
    /// Pro Linie ein Ring: [0] = aktueller Frame, dahinter die ausglühenden Ghosts.
    trails: Vec<VecDeque<Retained<CAShapeLayer>>>,
    sparkle: Retained<CALayer>,
    chip: Retained<CALayer>,
    chip_text: Retained<CATextLayer>,
    shown: Cell<bool>,
    /// Zuletzt angewendete (Position, Breite) — für Live-Updates aus der Vorschau.
    placed: Cell<Option<(OverlayPosition, u8)>>,
    /// Zuletzt angewendete Optik (Trail/Farben) — idempotente Live-Anwendung.
    style: std::cell::RefCell<Option<Style>>,
    /// Aktuelle Fensterbreite (Config-abhängig) für Pfad-Geometrie.
    width: Cell<f64>,
    started: Instant,
    mtm: MainThreadMarker,
}

impl Overlay {
    /// Erzeugt das (versteckte) Overlay-Fenster. Main-Thread.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let width = 320.0;
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, WIN_H));
        let window = unsafe {
            let w = NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            w.setReleasedWhenClosed(false);
            w.setOpaque(false);
            w.setBackgroundColor(Some(&NSColor::clearColor()));
            w.setHasShadow(false);
            w.setIgnoresMouseEvents(true);
            w.setLevel(WINDOW_LEVEL);
            w.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary,
            );
            w
        };

        let content = window.contentView().expect("contentView");
        content.setWantsLayer(true);
        // Ohne dieses Flag ignoriert AppKit CI-Filter auf Layern.
        content.setLayerUsesCoreImageFilters(true);
        let root = content.layer().expect("layer");
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, WIN_H));

        // Gruppe für alle Leuchtspuren; Level steuert die Gruppen-Opacity
        // (ein Call pro Frame statt 24 Layer-Updates).
        let group = CALayer::new();
        group.setFrame(bounds);
        root.addSublayer(&group);

        // Pro Farbe eine Leuchtspur: Slot 0 = aktueller Frame, dahinter Ghosts.
        // Widths/Opacities sind statisch pro Slot; pro Tick wandern nur die
        // Pfade einen Slot weiter (schnell — kein CI-Filter pro Ghost, kein
        // Gruppen-Blur: das hat vorher die Framerate gefressen).
        let trails: Vec<VecDeque<Retained<CAShapeLayer>>> = (0..N_RIBBONS)
            .map(|_| {
                (0..MAX_TRAIL)
                    .map(|age| {
                        let layer = CAShapeLayer::new();
                        unsafe {
                            layer.setFrame(bounds);
                            layer.setFillColor(None);
                            layer.setLineCap(kCALineCapRound);
                            layer.setHidden(true);
                            layer.setLineWidth(1.2 + age as f64 * 0.12);
                            // Additiv nur auf den zwei hellsten Slots — Überlagerungen
                            // leuchten auf, ohne dutzende teure Filter.
                            if age < 2
                                && let Some(dodge) = ci_filter_plain("CILinearDodgeBlendMode")
                            {
                                layer.setCompositingFilter(Some(&as_any(dodge)));
                            }
                        }
                        group.addSublayer(&layer);
                        layer
                    })
                    .collect()
            })
            .collect();

        // Glanzpunkt („Bling") am Wellenkamm der ersten Linie.
        let sparkle = CALayer::new();
        sparkle.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(8.0, 8.0)));
        sparkle.setBackgroundColor(Some(&cg_color(1.0, 1.0, 1.0, 0.9)));
        sparkle.setCornerRadius(4.0);
        sparkle.setHidden(true);
        group.addSublayer(&sparkle);

        // Status-Chip (transkribiert/✓/⚠): dunkle Pill + Text.
        let chip = CALayer::new();
        let chip_text = CATextLayer::new();
        unsafe {
            chip.setBackgroundColor(Some(&cg_color(0.06, 0.06, 0.09, 0.85)));
            chip.setCornerRadius(17.0);
            chip.setHidden(true);
            root.addSublayer(&chip);

            chip_text.setFontSize(14.0);
            chip_text.setAlignmentMode(objc2_quartz_core::kCAAlignmentCenter);
            chip_text.setContentsScale(2.0);
            chip.addSublayer(&chip_text);
        }

        let overlay = Self {
            window,
            group,
            trails,
            sparkle,
            chip,
            chip_text,
            shown: Cell::new(false),
            placed: Cell::new(None),
            style: std::cell::RefCell::new(None),
            width: Cell::new(width),
            started: Instant::now(),
            mtm,
        };
        overlay.layout(width);
        overlay
    }

    /// Farben/Trail aus der Config anwenden — idempotent, pro Tick aufrufbar.
    fn apply_style(&self, style: &Style) {
        if self.style.borrow().as_ref() == Some(style) {
            return;
        }
        for (i, trail) in self.trails.iter().enumerate() {
            let [r, g, b] = wave_color(&style.colors, i);
            let color = cg_color(
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
                1.0,
            );
            for (age, layer) in trail.iter().enumerate() {
                layer.setStrokeColor(Some(&color));
                layer.setOpacity(0.9 * style.decay.powi(age as i32));
            }
        }
        *self.style.borrow_mut() = Some(style.clone());
    }

    /// Ein Animations-Frame: Zustand lesen, Fenster + Layer nachziehen.
    /// Läuft per Timer auf dem Main-Thread (~60 fps); bei Hidden ein No-op.
    pub fn tick(&self, indicator: &Arc<Mutex<Indicator>>, config: &Arc<RwLock<Config>>) {
        // Optik-Parameter live aus der Config (Vorschau: Regler wirken sofort).
        let cfg_snapshot = config.read().map(|c| c.clone()).unwrap_or_default();
        let gain = cfg_snapshot.overlay_gain.clamp(2.0, 30.0);
        let speed = cfg_snapshot.overlay_speed.clamp(0.25, 3.0);
        let style = Style {
            trail_len: usize::from(cfg_snapshot.overlay_trail_len).clamp(1, MAX_TRAIL),
            decay: cfg_snapshot.overlay_trail_decay.clamp(0.2, 0.9),
            colors: cfg_snapshot.overlay_colors.clone(),
        };
        let (position, pct) = (
            cfg_snapshot.overlay_position,
            cfg_snapshot.overlay_width_pct,
        );

        let (phase, level) = {
            let Ok(mut ind) = indicator.lock() else {
                return;
            };
            ind.tick(Instant::now());
            if !ind.visible() {
                if self.shown.get() {
                    self.window.orderOut(None);
                    self.shown.set(false);
                }
                return;
            }
            (ind.phase().clone(), ind.smoothed_level(gain))
        };
        self.apply_style(&style);
        if !self.shown.get() || self.placed.get() != Some((position, pct)) {
            self.place_on_active_screen(position, pct);
            self.placed.set(Some((position, pct)));
            if !self.shown.get() {
                self.window.orderFrontRegardless();
                self.shown.set(true);
            }
        }

        // Implizite CA-Animationen aus — wir treiben die Frames selbst.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        match &phase {
            Phase::Recording => {
                let t = self.started.elapsed().as_secs_f32() * speed;
                self.render_waves(level, t, style.trail_len);
            }
            Phase::Preview => {
                // Synthetischer, atmender Pegel — kein Mikro nötig.
                let t = self.started.elapsed().as_secs_f32() * speed;
                let synth =
                    0.35 + 0.4 * (0.5 + 0.5 * (t * 1.6).sin()) * (0.7 + 0.3 * (t * 4.3).cos());
                self.render_waves(synth, t, style.trail_len);
            }
            Phase::Loading => {
                let dots = ".".repeat(1 + (self.started.elapsed().as_millis() / 400 % 3) as usize);
                self.show_chip(
                    &format!("Modelle werden geladen {dots}"),
                    (0.86, 0.86, 0.88),
                );
            }
            Phase::Ready => {
                self.show_chip("✓ bereit — Taste halten und sprechen", (0.47, 0.86, 0.55))
            }
            Phase::Transcribing => self.show_chip("… wird transkribiert", (0.86, 0.86, 0.88)),
            Phase::Done => self.show_chip("✓ eingefügt", (0.47, 0.86, 0.55)),
            Phase::Error(msg) => self.show_chip(&format!("⚠ {msg}"), (1.0, 0.63, 0.47)),
            Phase::Hidden => {}
        }
        CATransaction::commit();
    }

    /// Leuchtspuren zeichnen: Pfade wandern einen Slot weiter (ausglühen),
    /// nur Slot 0 wird neu berechnet; Level steuert die Gruppen-Helligkeit.
    /// `trail_len` = konfigurierte Spur-Länge (Rest der Slots bleibt versteckt).
    fn render_waves(&self, level: f32, t: f32, trail_len: usize) {
        self.chip.setHidden(true);
        self.group.setOpacity(0.45 + 0.55 * level);
        let width = self.width.get();
        for (i, trail) in self.trails.iter().enumerate() {
            for slot in (1..trail_len.min(trail.len())).rev() {
                trail[slot].setPath(trail[slot - 1].path().as_deref());
            }
            trail[0].setPath(Some(&line_path(level, t, i, width)));
            for (slot, layer) in trail.iter().enumerate() {
                layer.setHidden(slot >= trail_len);
            }
        }
        // Glanzpunkt auf dem Kamm der ersten Linie.
        let samples = wave_samples(level, t, 0, WAVE_POINTS);
        if let Some((idx, peak)) = samples
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        {
            let (x, y) = sample_to_point(idx, *peak, width);
            self.sparkle.setHidden(false);
            self.sparkle.setPosition(CGPoint::new(x, y));
            self.sparkle
                .setOpacity((0.2 + 0.7 * level) * (0.7 + 0.3 * (t * 7.0).sin()));
        }
    }

    fn show_chip(&self, text: &str, (r, g, b): (f64, f64, f64)) {
        for trail in &self.trails {
            for layer in trail {
                layer.setHidden(true);
            }
        }
        self.sparkle.setHidden(true);
        self.chip.setHidden(false);
        unsafe {
            self.chip_text.setString(Some(&*NSString::from_str(text)));
            self.chip_text
                .setForegroundColor(Some(&cg_color(r, g, b, 1.0)));
        }
    }

    /// Layer-Geometrie an (neue) Fensterbreite anpassen.
    fn layout(&self, width: f64) {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, WIN_H));
        self.group.setFrame(bounds);
        for trail in &self.trails {
            for layer in trail {
                layer.setFrame(bounds);
            }
        }
        let chip_w = 240.0_f64.min(width - 20.0);
        let chip_h = 34.0;
        self.chip.setFrame(CGRect::new(
            CGPoint::new((width - chip_w) / 2.0, (WIN_H - chip_h) / 2.0),
            CGSize::new(chip_w, chip_h),
        ));
        self.chip_text.setFrame(CGRect::new(
            CGPoint::new(0.0, 7.0),
            CGSize::new(chip_w, 20.0),
        ));
        self.width.set(width);
    }

    /// Oben/unten mittig auf dem Bildschirm der aktiven App, Breite aus Config.
    fn place_on_active_screen(&self, position: OverlayPosition, width_pct: u8) {
        let Some(screen) = NSScreen::mainScreen(self.mtm) else {
            return;
        };
        let vf = screen.visibleFrame();
        let width = (vf.size.width * f64::from(width_pct.clamp(15, 80)) / 100.0).max(160.0);
        if (width - self.width.get()).abs() > 0.5 {
            self.layout(width);
        }
        let x = vf.origin.x + (vf.size.width - width) / 2.0;
        let y = match position {
            OverlayPosition::Bottom => vf.origin.y + EDGE_MARGIN,
            OverlayPosition::Top => vf.origin.y + vf.size.height - WIN_H - EDGE_MARGIN,
        };
        self.window.setFrame_display(
            CGRect::new(CGPoint::new(x, y), CGSize::new(width, WIN_H)),
            false,
        );
    }
}

/// Offene Wellen-Linie als CGPath (Fenster-Koordinaten, y-Mitte = halbe Höhe).
fn line_path(level: f32, t: f32, layer: usize, width: f64) -> CFRetained<CGMutablePath> {
    let samples = wave_samples(level, t, layer, WAVE_POINTS);
    let path = CGMutablePath::new();
    for (i, y) in samples.iter().enumerate() {
        let (px, py) = sample_to_point(i, *y, width);
        unsafe {
            if i == 0 {
                CGMutablePath::move_to_point(Some(&path), ptr::null(), px, py);
            } else {
                CGMutablePath::add_line_to_point(Some(&path), ptr::null(), px, py);
            }
        }
    }
    path
}

fn sample_to_point(i: usize, y: f32, width: f64) -> (f64, f64) {
    let inner_w = width - 16.0;
    let px = 8.0 + inner_w * i as f64 / (WAVE_POINTS - 1) as f64;
    let py = WIN_H / 2.0 + f64::from(y) * (WIN_H / 2.0 - 6.0);
    (px, py)
}

fn ci_filter_plain(name: &str) -> Option<Retained<CIFilter>> {
    let filter = unsafe { CIFilter::filterWithName(&NSString::from_str(name)) }?;
    unsafe { filter.setDefaults() };
    Some(filter)
}

fn as_any(obj: Retained<CIFilter>) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(obj))
}

fn cg_color(r: f64, g: f64, b: f64, a: f64) -> Retained<CGColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a).CGColor()
}

// Kopplung sicherstellen: Ribbons == Wellen-Layer.
const _: () = assert!(N_RIBBONS == WAVE_LAYERS.len());

/// Farbe der Welle `i`: Config-Farbe, bei zu kurzer Liste (Config-Länge ≠ 4)
/// der Default derselben Position.
fn wave_color(colors: &[[u8; 3]], i: usize) -> [u8; 3] {
    colors
        .get(i)
        .copied()
        .unwrap_or(crate::config::DEFAULT_OVERLAY_COLORS[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_OVERLAY_COLORS;

    #[test]
    fn wave_color_falls_back_per_index_when_config_list_is_short() {
        let short = vec![[1, 2, 3]];
        assert_eq!(wave_color(&short, 0), [1, 2, 3]);
        for (i, default) in DEFAULT_OVERLAY_COLORS.iter().enumerate().skip(1) {
            assert_eq!(wave_color(&short, i), *default, "Index {i}");
        }
        assert_eq!(wave_color(&[], 0), DEFAULT_OVERLAY_COLORS[0], "leere Liste");
    }

    #[test]
    fn wave_color_ignores_surplus_entries() {
        let long: Vec<[u8; 3]> = (0..6).map(|i| [i, i, i]).collect();
        for i in 0..4 {
            assert_eq!(wave_color(&long, i), [i as u8, i as u8, i as u8]);
        }
    }
}
