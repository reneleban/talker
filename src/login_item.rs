//! Launch-at-Login via SMAppService (macOS 13+). Persistiert systemseitig —
//! Source of Truth ist der SMAppService-Status, nicht unsere Config.
//!
//! Funktioniert nur aus einem installierten .app-Bundle; aus `cargo run`
//! liefert macOS „NotFound" → in den Settings als Hinweis sichtbar.

use objc2_service_management::{SMAppService, SMAppServiceStatus};

use crate::error::{Result, TalkerError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    Enabled,
    Disabled,
    /// Registriert, aber vom Nutzer in den Systemeinstellungen zu bestätigen.
    RequiresApproval,
    /// Kein App-Bundle (z.B. `cargo run`) — Toggle nicht verfügbar.
    Unavailable,
}

pub fn status() -> LoginItemStatus {
    if !in_app_bundle() {
        return LoginItemStatus::Unavailable;
    }
    map_status(unsafe { SMAppService::mainAppService().status() })
}

/// Läuft der Prozess aus einem .app-Bundle (statt z.B. `cargo run`)?
fn in_app_bundle() -> bool {
    objc2_foundation::NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
}

/// Ein-/ausschalten; no-op wenn schon im Zielzustand.
pub fn set_enabled(wanted: bool) -> Result<()> {
    let current = status();
    if !needs_change(current, wanted) {
        return Ok(());
    }
    let service = unsafe { SMAppService::mainAppService() };
    let result = if wanted {
        unsafe { service.registerAndReturnError() }
    } else {
        unsafe { service.unregisterAndReturnError() }
    };
    result.map_err(|e| {
        TalkerError::Config(format!(
            "Launch-at-Login {}: {}",
            if wanted { "aktivieren" } else { "deaktivieren" },
            e.localizedDescription()
        ))
    })
}

fn map_status(raw: SMAppServiceStatus) -> LoginItemStatus {
    match raw {
        SMAppServiceStatus::Enabled => LoginItemStatus::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItemStatus::RequiresApproval,
        // NotFound liefert macOS auch für nie registrierte Apps — im Bundle
        // heißt das schlicht „aus"; Registrieren wird beim Toggle versucht.
        _ => LoginItemStatus::Disabled,
    }
}

/// Nur registrieren/deregistrieren, wenn der Zielzustand abweicht.
fn needs_change(current: LoginItemStatus, wanted: bool) -> bool {
    match current {
        LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval => !wanted,
        LoginItemStatus::Disabled => wanted,
        LoginItemStatus::Unavailable => false, // kein Bundle → nichts zu tun
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_status_maps_to_all_variants() {
        assert_eq!(
            map_status(SMAppServiceStatus::Enabled),
            LoginItemStatus::Enabled
        );
        assert_eq!(
            map_status(SMAppServiceStatus::NotRegistered),
            LoginItemStatus::Disabled
        );
        assert_eq!(
            map_status(SMAppServiceStatus::RequiresApproval),
            LoginItemStatus::RequiresApproval
        );
        // NotFound = nie registriert → als „aus" behandeln, Register versuchen.
        assert_eq!(
            map_status(SMAppServiceStatus::NotFound),
            LoginItemStatus::Disabled
        );
        assert_eq!(
            map_status(SMAppServiceStatus(99)),
            LoginItemStatus::Disabled
        );
    }

    #[test]
    fn toggle_only_acts_on_state_difference() {
        assert!(needs_change(LoginItemStatus::Disabled, true));
        assert!(needs_change(LoginItemStatus::Enabled, false));
        assert!(!needs_change(LoginItemStatus::Enabled, true));
        assert!(!needs_change(LoginItemStatus::Disabled, false));
        // Ausstehende Bestätigung + gewünschtes Aus → deregistrieren.
        assert!(needs_change(LoginItemStatus::RequiresApproval, false));
        // Ohne Bundle nie API-Aufrufe versuchen.
        assert!(!needs_change(LoginItemStatus::Unavailable, true));
        assert!(!needs_change(LoginItemStatus::Unavailable, false));
    }
}
