//! Injection: Text in die fokussierte Target App einfügen.
//! Ablauf: Clipboard sichern → Text setzen → Cmd+V simulieren → Clipboard wiederherstellen.

use std::thread;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::clipboard::{self, Pasteboard};
use crate::error::{Result, TalkerError};

const KEYCODE_V: u16 = 9;
/// Wartezeit, bis die Target App das Paste verarbeitet hat, bevor das
/// Clipboard wiederhergestellt wird.
const PASTE_SETTLE: Duration = Duration::from_millis(250);
/// Kurze Pause nach dem Hotkey-Release, damit dessen Modifier-Keyup nicht
/// mit dem simulierten Cmd+V kollidiert.
const RELEASE_SETTLE: Duration = Duration::from_millis(50);

/// bundle-id der aktuell fokussierten Target App (Kontext-Awareness,
/// Ticket-0026). None, wenn keine App frontmost ist oder sie keine
/// bundle-id hat (z.B. Kommandozeilen-Prozesse).
pub fn frontmost_bundle_id() -> Option<String> {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    app.bundleIdentifier().map(|s| s.to_string())
}

/// Eine laufende App fürs Regel-Editing (Ticket-0027): Klarname fürs UI,
/// bundle-id als kanonischer Matcher.
#[derive(Clone)]
pub struct RunningApp {
    pub name: String,
    pub bundle_id: String,
}

/// Laufende Apps mit Dock-Präsenz (ActivationPolicy::Regular), alphabetisch.
/// talker selbst (LSUIElement/Accessory) fällt durch den Filter heraus.
pub fn running_apps() -> Vec<RunningApp> {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    let mut apps: Vec<RunningApp> = workspace
        .runningApplications()
        .iter()
        .filter(|a| a.activationPolicy() == objc2_app_kit::NSApplicationActivationPolicy::Regular)
        .filter_map(|a| {
            Some(RunningApp {
                name: a.localizedName()?.to_string(),
                bundle_id: a.bundleIdentifier()?.to_string(),
            })
        })
        .collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);
    apps
}

/// Simuliert den Paste-Tastendruck in der Target App. Trait, damit der
/// Fehlerpfad in Tests injizierbar ist (CGEvent ist headless nicht testbar).
pub trait KeySender {
    fn send_cmd_v(&self) -> Result<()>;
}

/// Echtes Backend: CGEvent-Keyboard-Events auf HID-Ebene.
pub struct CgKeySender;

impl KeySender for CgKeySender {
    fn send_cmd_v(&self) -> Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|()| TalkerError::Keystroke("CGEventSource".into()))?;
        for key_down in [true, false] {
            let event = CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, key_down)
                .map_err(|()| TalkerError::Keystroke("CGEvent::new_keyboard_event".into()))?;
            event.set_flags(CGEventFlags::CGEventFlagCommand);
            event.post(CGEventTapLocation::HID);
        }
        Ok(())
    }
}

pub fn inject(pb: &dyn Pasteboard, keys: &dyn KeySender, text: &str) -> Result<()> {
    thread::sleep(RELEASE_SETTLE);
    let mut guard = RestoreGuard {
        pb,
        snapshot: Some(clipboard::save(pb)?),
    };
    pb.write_text(text)?;
    keys.send_cmd_v()?;
    thread::sleep(PASTE_SETTLE);
    guard.restore()
}

/// Stellt den Clipboard-Snapshot auf jedem Ausgangspfad wieder her: explizit
/// auf dem Erfolgspfad (Restore-Fehler propagierbar), via `Drop` auf jedem
/// Fehlerpfad (Restore-Fehler dort nur loggbar, nicht propagierbar).
struct RestoreGuard<'a> {
    pb: &'a dyn Pasteboard,
    snapshot: Option<clipboard::Snapshot>,
}

impl RestoreGuard<'_> {
    fn restore(&mut self) -> Result<()> {
        match self.snapshot.take() {
            Some(snapshot) => clipboard::restore(self.pb, snapshot),
            None => Ok(()),
        }
    }
}

impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            eprintln!("talker: Clipboard-Restore nach Injection-Fehler fehlgeschlagen: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::ItemData;
    use std::cell::RefCell;

    /// In-Memory-Fake mit NSPasteboard-Semantik (write leert zuerst);
    /// write_text kann auf Fehler geschaltet werden.
    struct FakePasteboard {
        items: RefCell<Vec<ItemData>>,
        fail_write_text: bool,
    }

    impl FakePasteboard {
        fn with_content(text: &str) -> Self {
            Self {
                items: RefCell::new(vec![text_item(text)]),
                fail_write_text: false,
            }
        }
    }

    impl Pasteboard for FakePasteboard {
        fn read_items(&self) -> Result<Vec<ItemData>> {
            Ok(self.items.borrow().clone())
        }
        fn write_items(&self, items: &[ItemData]) -> Result<()> {
            *self.items.borrow_mut() = items.to_vec();
            Ok(())
        }
        fn write_text(&self, text: &str) -> Result<()> {
            if self.fail_write_text {
                return Err(TalkerError::Clipboard("write_text fehlgeschlagen".into()));
            }
            *self.items.borrow_mut() = vec![text_item(text)];
            Ok(())
        }
    }

    struct FakeKeySender {
        fail: bool,
    }

    impl KeySender for FakeKeySender {
        fn send_cmd_v(&self) -> Result<()> {
            if self.fail {
                return Err(TalkerError::Keystroke("send_cmd_v fehlgeschlagen".into()));
            }
            Ok(())
        }
    }

    fn text_item(text: &str) -> ItemData {
        vec![(
            "public.utf8-plain-text".to_string(),
            text.as_bytes().to_vec(),
        )]
    }

    #[test]
    fn happy_path_injects_and_restores_clipboard() {
        let pb = FakePasteboard::with_content("Nutzer-Inhalt");
        let keys = FakeKeySender { fail: false };

        inject(&pb, &keys, "diktierter Text").unwrap();

        assert_eq!(pb.read_items().unwrap(), vec![text_item("Nutzer-Inhalt")]);
    }

    #[test]
    fn send_cmd_v_error_still_restores_clipboard() {
        let pb = FakePasteboard::with_content("Nutzer-Inhalt");
        let keys = FakeKeySender { fail: true };

        let result = inject(&pb, &keys, "diktierter Text");

        assert!(result.is_err());
        assert_eq!(
            pb.read_items().unwrap(),
            vec![text_item("Nutzer-Inhalt")],
            "Clipboard muss trotz send_cmd_v-Fehler restauriert sein"
        );
    }

    #[test]
    fn write_text_error_still_restores_clipboard() {
        let pb = FakePasteboard {
            items: RefCell::new(vec![text_item("Nutzer-Inhalt")]),
            fail_write_text: true,
        };
        let keys = FakeKeySender { fail: false };

        let result = inject(&pb, &keys, "diktierter Text");

        assert!(result.is_err());
        assert_eq!(
            pb.read_items().unwrap(),
            vec![text_item("Nutzer-Inhalt")],
            "Clipboard muss trotz write_text-Fehler restauriert sein"
        );
    }
}
