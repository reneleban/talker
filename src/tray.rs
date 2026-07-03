//! Menüleisten-Icon (tray-icon/NSStatusItem): Status idle/aktiv/Warnung,
//! Modus-Kürzel im Icon + Menü mit Modus-Schnellwechsel/Einstellungen/Beenden.
//!
//! Icons sind programmatisch gezeichnete Mikrofon-Glyphen. idle/Warnung als
//! macOS-Template (monochrom, passt sich hell/dunkel an), Aufnahme bewusst rot.
//! Das Modus-Kürzel (R/G/N/L) wird als eingebettete Pixel-Glyphe gerastert.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::cleanup::CleanupMode;
use crate::error::{Result, TalkerError};
use crate::indicator::Phase;

pub struct Tray {
    icon: TrayIcon,
    pub settings_id: MenuId,
    pub quit_id: MenuId,
    /// Modus-Menüeinträge (Checkmark-Auswahl) in `CleanupMode::ALL`-Reihenfolge.
    mode_items: Vec<(MenuId, CleanupMode, CheckMenuItem)>,
    mode: Cell<CleanupMode>,
    /// Setup läuft (Modell-Download, Ticket-0029): idle zeigt dann das
    /// durchgestrichene Mikrofon, bis das Setup fertig ist.
    setup: Cell<bool>,
    /// Aus der Indicator-Phase abgeleiteter Aufnahme-Zustand (Ticket-0035) —
    /// nur Cache für Idempotenz, Besitzer ist der Indicator.
    recording: Cell<Option<CleanupMode>>,
}

impl Tray {
    /// Muss auf dem Main-Thread laufen (NSStatusItem).
    pub fn new(initial_mode: CleanupMode) -> Result<Self> {
        let map_err = |e: &dyn std::fmt::Display| TalkerError::Tray(e.to_string());
        let menu = Menu::new();

        let mode_items: Vec<(MenuId, CleanupMode, CheckMenuItem)> = CleanupMode::ALL
            .into_iter()
            .map(|mode| {
                let item = CheckMenuItem::new(mode.label(), true, mode == initial_mode, None);
                let id = item.id().clone();
                (id, mode, item)
            })
            .collect();
        for (_, _, item) in &mode_items {
            menu.append(item).map_err(|e| map_err(&e))?;
        }
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| map_err(&e))?;

        let settings = MenuItem::new("Einstellungen…", true, None);
        let quit = MenuItem::new("Beenden", true, None);
        menu.append_items(&[&settings, &quit])
            .map_err(|e| map_err(&e))?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(mic_icon(MicStyle::Idle, Some(initial_mode)))
            .with_icon_as_template(true)
            .build()
            .map_err(|e| map_err(&e))?;
        Ok(Self {
            icon,
            settings_id: settings.id().clone(),
            quit_id: quit.id().clone(),
            mode_items,
            mode: Cell::new(initial_mode),
            setup: Cell::new(false),
            recording: Cell::new(None),
        })
    }

    /// Setup-Zustand an/aus (Ticket-0029). Wirkt auf alle idle-Zeichnungen,
    /// damit z.B. ein Modus-Sync das Setup-Icon nicht überschreibt.
    pub fn set_setup(&self, on: bool) {
        self.setup.set(on);
        self.set_idle();
    }

    /// Gehört die Menü-ID zu einem Modus-Eintrag?
    pub fn mode_for_id(&self, id: &MenuId) -> Option<CleanupMode> {
        self.mode_items
            .iter()
            .find(|(item_id, _, _)| item_id == id)
            .map(|(_, mode, _)| *mode)
    }

    /// Tray auf den (extern geänderten) Modus bringen — idempotent, damit die
    /// UI das pro Frame aufrufen kann (eine Quelle der Wahrheit: die Config).
    pub fn sync_mode(&self, mode: CleanupMode) {
        if self.mode.get() == mode {
            return;
        }
        self.mode.set(mode);
        for (checked, (_, item_mode, item)) in checked_flags(mode).into_iter().zip(&self.mode_items)
        {
            debug_assert_eq!(checked, *item_mode == mode);
            item.set_checked(checked);
        }
        self.set_idle();
    }

    /// Aufnahme-Status aus der Indicator-Phase ableiten (Ticket-0035) —
    /// idempotent, damit die UI das pro Frame aufrufen kann. Der Indicator ist
    /// der einzige Besitzer des Zustands; das Badge zeigt während der Aufnahme
    /// den für diese Utterance AUFGELÖSTEN Modus (Kontext-Awareness, Ticket-0026).
    pub fn sync_recording(&self, phase: &Phase) {
        let recording = recording_mode(phase);
        if self.recording.get() == recording {
            return;
        }
        self.recording.set(recording);
        match recording {
            Some(resolved_mode) => {
                if let Err(e) = self
                    .icon
                    .set_icon(Some(mic_icon(MicStyle::Recording, Some(resolved_mode))))
                {
                    eprintln!("talker: Tray-Icon (aufnehmend) nicht setzbar: {e}");
                }
                self.icon.set_icon_as_template(false); // bewusst rot, soll auffallen
            }
            None => self.set_idle(),
        }
    }

    fn set_idle(&self) {
        let (style, badge) = if self.setup.get() {
            (MicStyle::Setup, None)
        } else {
            (MicStyle::Idle, Some(self.mode.get()))
        };
        if let Err(e) = self.icon.set_icon(Some(mic_icon(style, badge))) {
            eprintln!("talker: Tray-Icon (idle) nicht setzbar: {e}");
        }
        self.icon.set_icon_as_template(true);
    }

    pub fn set_permission_warning(&self) {
        if let Err(e) = self.icon.set_icon(Some(mic_icon(MicStyle::Warning, None))) {
            eprintln!("talker: Tray-Icon (Warnung) nicht setzbar: {e}");
        }
        self.icon.set_icon_as_template(true);
    }

    /// Warnung aufheben (z.B. Event-Tap nachträglich installiert) → idle.
    pub fn clear_permission_warning(&self) {
        self.set_idle();
    }
}

/// Reine Ableitung MicStyle-Quelle: nur `Phase::Recording` färbt das Tray —
/// Preview (Einstellungen) und alle Verarbeitungs-Phasen bleiben idle.
fn recording_mode(phase: &Phase) -> Option<CleanupMode> {
    match phase {
        Phase::Recording { mode } => Some(*mode),
        _ => None,
    }
}

/// Checkmark-Zustände der Modus-Einträge (Reihenfolge = `CleanupMode::ALL`).
fn checked_flags(active: CleanupMode) -> [bool; 4] {
    let mut flags = [false; 4];
    for (i, mode) in CleanupMode::ALL.into_iter().enumerate() {
        flags[i] = mode == active;
    }
    flags
}

/// Kürzel des Modus fürs Icon.
pub fn mode_badge(mode: CleanupMode) -> char {
    match mode {
        CleanupMode::Raw => 'R',
        CleanupMode::Business => 'G',
        CleanupMode::Casual => 'N',
        CleanupMode::LlmOptimized => 'L',
    }
}

/// 4×7-Pixel-Glyphen für die Modus-Kürzel (1 = Tinte).
fn badge_bitmap(letter: char) -> Option<&'static [u8; 7]> {
    // Jede Zeile ist ein Nibble (Bit 3 = linkeste Spalte).
    match letter {
        'R' => Some(&[0b1110, 0b1001, 0b1001, 0b1110, 0b1010, 0b1001, 0b1001]),
        'G' => Some(&[0b0111, 0b1000, 0b1000, 0b1011, 0b1001, 0b1001, 0b0111]),
        'N' => Some(&[0b1001, 0b1101, 0b1101, 0b1011, 0b1011, 0b1001, 0b1001]),
        'L' => Some(&[0b1000, 0b1000, 0b1000, 0b1000, 0b1000, 0b1000, 0b1111]),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MicStyle {
    Idle,
    Recording,
    Warning,
    /// Setup läuft: Mikrofon diagonal durchgestrichen (Ticket-0029).
    Setup,
}

/// Rastert das Mikrofon-Glyph (22 pt @2x = 44 px) per Signed-Distance-Field,
/// optional mit Modus-Kürzel oben rechts.
fn mic_icon(style: MicStyle, badge: Option<CleanupMode>) -> Icon {
    const S: usize = 44;
    let color: [u8; 3] = match style {
        MicStyle::Recording => [255, 69, 58], // macOS systemRed
        _ => [0, 0, 0],                       // Template: nur Alpha zählt
    };
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut a = glyph_alpha(px, py);
            if style == MicStyle::Warning {
                a = a.max(bang_alpha(px, py));
            }
            if style == MicStyle::Setup {
                a = a.max(strike_alpha(px, py));
            }
            if a > 0.0 {
                let i = (y * S + x) * 4;
                rgba[i..i + 3].copy_from_slice(&color);
                rgba[i + 3] = (a * 255.0) as u8;
            }
        }
    }
    // Modus-Kürzel: 4×7-Pixelglyphe, 3× skaliert, oben rechts (Warnung hat Vorrang).
    if style != MicStyle::Warning
        && let Some(bitmap) = badge.and_then(|m| badge_bitmap(mode_badge(m)))
    {
        const SCALE: usize = 3;
        const OFF_X: usize = 31;
        const OFF_Y: usize = 3;
        for (row, bits) in bitmap.iter().enumerate() {
            for col in 0..4 {
                if bits & (0b1000 >> col) == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let (x, y) = (OFF_X + col * SCALE + dx, OFF_Y + row * SCALE + dy);
                        let i = (y * S + x) * 4;
                        rgba[i..i + 3].copy_from_slice(&color);
                        rgba[i + 3] = 255;
                    }
                }
            }
        }
    }
    // from_rgba schlägt nur bei falscher Puffergröße fehl — hier konstant korrekt.
    Icon::from_rgba(rgba, S as u32, S as u32).expect("Icon-Puffergröße")
}

/// Mikrofon: Kapsel + Bügel + Stiel + Fuß, als SDF mit 1-px-Kantenglättung.
fn glyph_alpha(x: f32, y: f32) -> f32 {
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
    let d = capsule.min(holder).min(stem).min(base);
    (0.5 - d).clamp(0.0, 1.0)
}

/// Diagonaler Durchstreich-Balken (für den Setup-Zustand).
fn strike_alpha(x: f32, y: f32) -> f32 {
    let d = sd_segment(x, y, 7.0, 7.0, 37.0, 37.0) - 2.0;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Ausrufezeichen oben rechts (für den Warnung-Zustand).
fn bang_alpha(x: f32, y: f32) -> f32 {
    let line = sd_segment(x, y, 36.0, 5.0, 36.0, 12.0) - 1.8;
    let dot = ((x - 36.0).powi(2) + (y - 17.5).powi(2)).sqrt() - 2.2;
    (0.5 - line.min(dot)).clamp(0.0, 1.0)
}

/// Abstand des Punkts (x,y) zur Strecke (ax,ay)-(bx,by).
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

thread_local! {
    /// Nur auf dem Main-Thread gesetzt. Erlaubt Send-'static-Callbacks
    /// (CGEventTap), die auf dem Main-RunLoop laufen, den Status-Zugriff.
    static INSTANCE: RefCell<Option<Rc<Tray>>> = const { RefCell::new(None) };
}

pub fn set_instance(tray: Rc<Tray>) {
    INSTANCE.with(|i| *i.borrow_mut() = Some(tray));
}

/// Führt `f` mit dem Tray aus, sofern auf dem Main-Thread registriert;
/// auf anderen Threads ein No-op.
pub fn with_instance(f: impl FnOnce(&Tray)) {
    INSTANCE.with(|i| {
        if let Some(tray) = i.borrow().as_ref() {
            f(tray);
        }
    });
}

/// Wie `with_instance`, aber mit Rückgabewert (None ohne Instanz/Main-Thread).
pub fn with_instance_map<R>(f: impl FnOnce(&Tray) -> R) -> Option<R> {
    INSTANCE.with(|i| i.borrow().as_ref().map(|tray| f(tray)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_badges_are_distinct_and_have_bitmaps() {
        let mut badges: Vec<char> = CleanupMode::ALL.iter().map(|m| mode_badge(*m)).collect();
        assert_eq!(badges, ['R', 'G', 'N', 'L']);
        badges.sort_unstable();
        badges.dedup();
        assert_eq!(badges.len(), 4, "Kürzel müssen eindeutig sein");
        for badge in badges {
            let bitmap = badge_bitmap(badge).expect("Bitmap fehlt");
            assert!(bitmap.iter().any(|row| *row != 0), "{badge}: leere Glyphe");
            assert!(
                bitmap.iter().all(|row| *row <= 0b1111),
                "{badge}: >4 Spalten"
            );
        }
        assert!(badge_bitmap('X').is_none());
    }

    /// AK1 (Ticket-0035): vollständiges Phase→Aufnahme-Mapping — genau
    /// `Phase::Recording` färbt das Tray, mit dem aufgelösten Modus als Badge;
    /// alle anderen Phasen (inkl. Preview) sind idle.
    #[test]
    fn recording_derives_only_from_the_recording_phase() {
        for mode in CleanupMode::ALL {
            assert_eq!(recording_mode(&Phase::Recording { mode }), Some(mode));
        }
        for phase in [
            Phase::Hidden,
            Phase::Loading,
            Phase::Ready,
            Phase::Preview,
            Phase::Transcribing,
            Phase::Done,
            Phase::Error("kaputt".into()),
        ] {
            assert_eq!(recording_mode(&phase), None, "{phase:?}");
        }
    }

    #[test]
    fn checked_flags_mark_exactly_the_active_mode() {
        for (i, mode) in CleanupMode::ALL.into_iter().enumerate() {
            let flags = checked_flags(mode);
            assert_eq!(flags.iter().filter(|f| **f).count(), 1, "{mode:?}");
            assert!(flags[i], "{mode:?} an falscher Position");
        }
    }

    #[test]
    fn glyph_has_ink_inside_and_none_outside() {
        // Kapsel-Mitte voll deckend, Ecken leer.
        assert_eq!(glyph_alpha(22.0, 15.0), 1.0);
        assert_eq!(glyph_alpha(2.0, 2.0), 0.0);
        assert_eq!(glyph_alpha(42.0, 42.0), 0.0);
        // Bügel nur unterhalb der Kapselmitte.
        assert_eq!(glyph_alpha(12.0, 14.0), 0.0);
        assert!(glyph_alpha(12.2, 21.5) > 0.5);
    }

    #[test]
    fn setup_strike_crosses_the_glyph_diagonally() {
        // Auf der Diagonale voll deckend — auch außerhalb der Mikrofon-Tinte.
        assert!(strike_alpha(10.0, 10.0) > 0.5);
        assert!(strike_alpha(22.0, 22.0) > 0.5);
        assert!(strike_alpha(34.0, 34.0) > 0.5);
        assert_eq!(glyph_alpha(10.0, 10.0), 0.0, "kreuzt freie Fläche");
        // Abseits der Diagonale keine Streich-Tinte.
        assert_eq!(strike_alpha(36.0, 8.0), 0.0);
        assert_eq!(strike_alpha(8.0, 36.0), 0.0);
    }

    #[test]
    fn warning_bang_is_separate_from_glyph() {
        assert_eq!(glyph_alpha(36.0, 8.0), 0.0);
        assert!(bang_alpha(36.0, 8.0) > 0.5);
        assert!(bang_alpha(36.0, 17.5) > 0.5);
        assert_eq!(
            bang_alpha(36.0, 14.5),
            0.0,
            "Lücke zwischen Strich und Punkt"
        );
    }
}
