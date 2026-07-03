//! System-Permissions: Accessibility (CGEventTap + Cmd+V) und Mikrofon (Aufnahme).

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
    -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

/// Accessibility-Status ohne System-Prompt (für Status-Anzeigen).
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Prüft die Accessibility-Permission; zeigt beim ersten Mal den System-Prompt.
pub fn ensure_accessibility() -> bool {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options =
        CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    /// macOS zeigt den Prompt beim ersten Aufnahme-Versuch.
    Undetermined,
    Denied,
}

/// Anzeige-Modell einer Permission-Zeile (pure, unit-testbar — AK1 Ticket-0009).
#[derive(Debug, PartialEq, Eq)]
pub struct PermissionRow {
    pub granted: bool,
    pub label: &'static str,
    /// Erklärung, nur wenn Handlungs-/Wartebedarf besteht.
    pub hint: Option<&'static str>,
    /// Anker der Systemeinstellungen, wenn der Nutzer aktiv werden muss.
    pub pane: Option<&'static str>,
}

/// Bildet die Permission-Zustände auf die Onboarding-Zeilen ab.
pub fn permission_rows(accessibility: bool, mic: MicPermission) -> [PermissionRow; 2] {
    let ax = if accessibility {
        PermissionRow {
            granted: true,
            label: "Bedienungshilfen",
            hint: None,
            pane: None,
        }
    } else {
        PermissionRow {
            granted: false,
            label: "Bedienungshilfen",
            hint: Some(
                "Nötig für den globalen Hotkey und das Einfügen (Cmd+V). \
                 Nach dem Erteilen startet talker sich einmal selbst neu.",
            ),
            pane: Some("Privacy_Accessibility"),
        }
    };
    let mic = match mic {
        MicPermission::Granted => PermissionRow {
            granted: true,
            label: "Mikrofon",
            hint: None,
            pane: None,
        },
        MicPermission::Undetermined => PermissionRow {
            granted: false,
            label: "Mikrofon",
            hint: Some("macOS fragt beim ersten Diktat — einfach lossprechen."),
            pane: None,
        },
        MicPermission::Denied => PermissionRow {
            granted: false,
            label: "Mikrofon",
            hint: Some("Ohne Mikrofon kein Diktat — bitte in den Systemeinstellungen erlauben."),
            pane: Some("Privacy_Microphone"),
        },
    };
    [ax, mic]
}

/// Fragt die Mikrofon-Permission aktiv an (System-Dialog, Ticket-0030).
/// Ohne diesen Call käme der Prompt erst beim ersten Aufnahme-Versuch —
/// der setzt den Hotkey voraus, der wiederum Accessibility (+ Relaunch).
/// Fire-and-forget: die UI pollt den Status pro Frame, die Completion
/// (beliebige Dispatch-Queue) loggt nur.
pub fn request_microphone() {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
    let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
        return;
    };
    let handler = block2::RcBlock::new(|granted: objc2::runtime::Bool| {
        eprintln!(
            "talker: Mikrofon-Permission {}.",
            if granted.as_bool() {
                "erteilt"
            } else {
                "abgelehnt"
            }
        );
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler) };
}

/// Braucht talker einen Self-Relaunch, damit der Event-Tap funktioniert?
/// macOS/TCC cached die Accessibility-Entscheidung pro Prozess: wurde sie
/// erst zur Laufzeit erteilt, scheitert `CGEventTap::new` bis zum Neustart.
/// Loop-Guard: war sie schon beim Start erteilt und der Tap scheitert
/// trotzdem, hilft ein Relaunch nicht — dann sichtbarer Fehler statt Schleife.
pub fn should_relaunch_for_tap(had_accessibility_at_start: bool, granted_now: bool) -> bool {
    !had_accessibility_at_start && granted_now
}

/// Status der Mikrofon-Permission (TCC).
pub fn microphone_status() -> MicPermission {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
        return MicPermission::Denied;
    };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    match status {
        AVAuthorizationStatus::Authorized => MicPermission::Granted,
        AVAuthorizationStatus::NotDetermined => MicPermission::Undetermined,
        _ => MicPermission::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granted_rows_need_no_action() {
        let [ax, mic] = permission_rows(true, MicPermission::Granted);
        assert!(ax.granted && mic.granted);
        assert!(ax.hint.is_none() && ax.pane.is_none());
        assert!(mic.hint.is_none() && mic.pane.is_none());
    }

    #[test]
    fn missing_accessibility_links_to_settings_pane() {
        let [ax, _] = permission_rows(false, MicPermission::Granted);
        assert!(!ax.granted);
        assert_eq!(ax.pane, Some("Privacy_Accessibility"));
        assert!(ax.hint.is_some());
    }

    #[test]
    fn undetermined_mic_explains_but_needs_no_settings_visit() {
        let [_, mic] = permission_rows(true, MicPermission::Undetermined);
        assert!(!mic.granted);
        assert!(mic.hint.is_some());
        assert_eq!(mic.pane, None, "Prompt kommt von selbst — kein Link nötig");
    }

    #[test]
    fn denied_mic_links_to_settings_pane() {
        let [_, mic] = permission_rows(true, MicPermission::Denied);
        assert!(!mic.granted);
        assert_eq!(mic.pane, Some("Privacy_Microphone"));
    }

    /// Relaunch nur nach Laufzeit-Grant (AK 2), nie als Schleife (AK 3).
    #[test]
    fn relaunch_only_after_runtime_grant_never_loops() {
        // Beim Start fehlend, jetzt erteilt → Relaunch (TCC-Cache umgehen).
        assert!(should_relaunch_for_tap(false, true));
        // Beim Start schon erteilt, Tap scheitert trotzdem → KEIN Relaunch
        // (sonst Endlos-Schleife: der neue Prozess sähe dieselbe Lage).
        assert!(!should_relaunch_for_tap(true, true));
        // Noch nicht erteilt → weiter warten, kein Relaunch.
        assert!(!should_relaunch_for_tap(false, false));
        assert!(!should_relaunch_for_tap(true, false));
    }
}
