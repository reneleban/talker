//! Globaler PTT-Hotkey via CGEventTap (flagsChanged, listen-only).
//! Die PTT-Taste kommt zur Laufzeit aus der Config (Ticket-0006) —
//! Änderungen wirken ohne Tap-Neuinstallation.

use std::cell::Cell;
use std::sync::{Arc, RwLock};

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};

use crate::config::Config;
use crate::error::{Result, TalkerError};

pub const DEFAULT_PTT_KEYCODE: i64 = 63; // kVK_Function (Fn/🌐)

/// Wählbare PTT-Tasten (Keycode, Label) — Modifier, die als flagsChanged kommen.
pub const SELECTABLE_KEYS: [(i64, &str); 6] = [
    (63, "Fn / 🌐"),
    (61, "Option rechts"),
    (58, "Option links"),
    (54, "Cmd rechts"),
    (62, "Ctrl rechts"),
    (60, "Shift rechts"),
];

/// Modifier-Flag, das bei flagsChanged anzeigt, ob die Taste gedrückt ist.
fn modifier_flag(keycode: i64) -> CGEventFlags {
    match keycode {
        58 | 61 => CGEventFlags::CGEventFlagAlternate, // Option links/rechts
        63 => CGEventFlags::CGEventFlagSecondaryFn,    // Fn/🌐
        54 | 55 => CGEventFlags::CGEventFlagCommand,   // Cmd rechts/links
        56 | 60 => CGEventFlags::CGEventFlagShift,     // Shift links/rechts
        59 | 62 => CGEventFlags::CGEventFlagControl,   // Ctrl links/rechts
        _ => CGEventFlags::CGEventFlagNonCoalesced,    // unbekannt → nie gedrückt
    }
}

/// Installiert den Event-Tap auf dem aktuellen (Main-)RunLoop.
/// Die PTT-Taste wird pro Event aus `config` gelesen.
/// Gibt den Tap zurück — der Aufrufer muss ihn am Leben halten.
pub fn install(
    config: Arc<RwLock<Config>>,
    on_press: impl Fn() + Send + 'static,
    on_release: impl Fn() + Send + 'static,
) -> Result<CGEventTap<'static>> {
    let held = Cell::new(false);
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged],
        move |_proxy, _event_type, event| {
            let keycode = config
                .read()
                .map(|c| c.hotkey_keycode)
                .unwrap_or(DEFAULT_PTT_KEYCODE);
            if event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) == keycode {
                let pressed = event.get_flags().contains(modifier_flag(keycode));
                if pressed && !held.get() {
                    held.set(true);
                    on_press();
                } else if !pressed && held.get() {
                    held.set(false);
                    on_release();
                }
            }
            CallbackResult::Keep
        },
    )
    .map_err(|()| TalkerError::EventTap)?;

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|()| TalkerError::EventTap)?;
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    Ok(tap)
}
