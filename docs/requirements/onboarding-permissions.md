# Anforderungen — Settings, Erst-Start-Onboarding, Permissions, Login-Item

EARS-Format nach [`docs/requirements/README.md`](README.md). Quellen: Ist-Zustand
im Code (`src/permissions.rs`, `src/ui.rs`, `src/login_item.rs`, `src/main.rs`),
Akzeptanzkriterien aus `docs/ticket/ticket-0006/-0008/-0009/-0030` als Ausgangspunkt
— wo Code und Ticket-AK auseinanderliefen, gilt der Code. Zitate verweisen auf
Funktionen, nicht auf Zeilennummern (bleiben über Refactors hinweg stabil).

## Erst-Start-Onboarding

- **REQ-ONBOARD-001**: WENN talker startet und der Mikrofon-Permission-Status
  `Undetermined` ist, soll das System sofort und unabhängig vom
  Accessibility-Status den System-Mikrofon-Prompt anfordern.
  (`src/main.rs:main`, `src/permissions.rs:request_microphone`, Ticket-0030 AK1)
- **REQ-ONBOARD-002**: WENN beim Start die Accessibility-Permission fehlt ODER
  die Mikrofon-Permission verweigert (`Denied`) ist ODER das STT-Modell noch
  nicht bereit ist, soll das System das Settings-Fenster sichtbar öffnen.
  (`src/main.rs:main` — `show_onboarding`)
- **REQ-ONBOARD-003**: WÄHREND beim Start Accessibility erteilt, Mikrofon nicht
  verweigert und das STT-Modell bereit ist, soll das System das
  Settings-Fenster unsichtbar starten (kein Onboarding nötig).
  (`src/main.rs:main` — `show_onboarding`, `src/ui.rs:SettingsApp::logic` — `hide_on_start`)
- **REQ-ONBOARD-004**: WÄHREND das STT-Modell (Parakeet) noch nicht bereit ist,
  soll das System im Settings-Fenster anstelle der regulären Tabs die
  Erst-Start-Einrichtung anzeigen (Lizenz-Consent → Download-Fortschritt →
  ggf. Fehleranzeige). (`src/ui.rs:SettingsApp::ui` — Setup-Gate, `src/ui.rs:setup_stage`)
- **REQ-ONBOARD-005**: WENN der Nutzer im Consent-Schritt „Lizenzen akzeptieren
  und Modelle laden" klickt, soll das System den Modell-Download-Consent in
  der Config persistieren und die nötigen Modell-Downloads starten.
  (`src/ui.rs:SettingsApp::setup_view` — `SetupStage::Consent`)
- **REQ-ONBOARD-006**: FALLS der Parakeet-Modell-Download fehlschlägt, DANN
  soll das System die Fehlermeldung sichtbar anzeigen und einen
  „Erneut versuchen"-Button anbieten, der den Download neu startet.
  (`src/ui.rs:SettingsApp::setup_view` — `SetupStage::Failed`)
- **REQ-ONBOARD-007**: WÄHREND das STT-Modell nicht bereit ist (Setup-Gate
  aktiv), soll das System einen Push-to-talk-Tastendruck blockieren, statt
  eine Aufnahme zu starten. (`src/pipeline.rs:PttSession::press`, Test
  `press_blocked_by_setup_gate_never_touches_audio`)

## Accessibility & Mikrofon-Permission

- **REQ-ONBOARD-008**: Das System soll beim Start den Accessibility-Status
  prüfen und dabei, falls noch nicht entschieden, den System-Trust-Prompt
  auslösen (`AXIsProcessTrustedWithOptions`). (`src/permissions.rs:ensure_accessibility`)
- **REQ-ONBOARD-009**: Das System soll für Accessibility und Mikrofon je eine
  Anzeige-Zeile mit Status-Punkt (erteilt/fehlt), Erklärungstext bei
  Handlungsbedarf und, wo ein Nutzer aktiv werden muss, einem Link in die
  passende Systemeinstellungen-Seite liefern. (`src/permissions.rs:permission_rows`,
  `src/ui.rs:SettingsApp::permissions_section`)
- **REQ-ONBOARD-010**: FALLS die Accessibility-Permission fehlt, DANN soll die
  Anzeige-Zeile einen Link zum Bedienungshilfen-Pane der Systemeinstellungen
  liefern, aber keinen Hinweis „bitte in Systemeinstellungen erlauben"
  suggerieren, der ohne Aktion verschwindet. (`src/permissions.rs:permission_rows`,
  Pane `Privacy_Accessibility`)
- **REQ-ONBOARD-011**: FALLS der Mikrofon-Status `Undetermined` ist, DANN soll
  die Anzeige-Zeile erklären, dass macOS den Prompt selbst zeigt, ohne einen
  Systemeinstellungen-Link anzubieten (kein Ziel, das der Nutzer manuell
  ansteuern müsste). (`src/permissions.rs:permission_rows`, Test
  `undetermined_mic_explains_but_needs_no_settings_visit`)
- **REQ-ONBOARD-012**: FALLS die Mikrofon-Permission verweigert (`Denied`)
  ist, DANN soll die Anzeige-Zeile einen Link zum Mikrofon-Pane der
  Systemeinstellungen liefern. (`src/permissions.rs:permission_rows`, Pane
  `Privacy_Microphone`, Test `denied_mic_links_to_settings_pane`)
- **REQ-ONBOARD-013**: WENN die Accessibility-Permission zur Laufzeit erteilt
  wird, nachdem sie beim App-Start noch fehlte, UND der Event-Tap weiterhin
  scheitert, soll das System sich selbst neu starten (Self-Relaunch), weil
  macOS/TCC die Accessibility-Entscheidung pro Prozess cacht und der Tap erst
  im frischen Prozess funktioniert. (`src/permissions.rs:should_relaunch_for_tap`,
  `src/main.rs:relaunch`, `src/main.rs:main` — Retry-Timer, Ticket-0030 AK2)
- **REQ-ONBOARD-014**: FALLS die Accessibility-Permission bereits beim
  App-Start erteilt war und die Event-Tap-Installation trotzdem scheitert,
  DANN soll das System KEINEN Self-Relaunch auslösen (Loop-Guard: ein neuer
  Prozess sähe dieselbe Ausgangslage), sondern den Fehler sichtbar lassen
  (Tray-Warnhinweis). (`src/permissions.rs:should_relaunch_for_tap`, Test
  `relaunch_only_after_runtime_grant_never_loops`, Ticket-0030 AK3)
- **REQ-ONBOARD-015**: FALLS die Event-Tap-Installation beim Start scheitert
  (z. B. Accessibility fehlt noch), DANN soll das System alle 3 Sekunden einen
  erneuten Installationsversuch unternehmen, statt einen manuellen Neustart
  zu verlangen. (`src/main.rs:main` — `install_tap`-Factory, 3-s-`NSTimer`-Retry)
- **REQ-ONBOARD-016**: WÄHREND ein Modell-Download läuft, soll ein
  automatischer Self-Relaunch (REQ-ONBOARD-013) unterbleiben, damit ein
  laufender Download (bis zu mehrere GB) nicht abgebrochen wird.
  (`src/main.rs:main` — Retry-Timer, `src/models.rs:ModelsState::any_download_running`)
- **REQ-ONBOARD-017**: FALLS die Accessibility-Permission fehlt, DANN soll das
  System einen Warnhinweis im Tray-Icon setzen und diesen erst wieder
  entfernen, wenn der Event-Tap erfolgreich installiert ist. (`src/main.rs:main`,
  `src/tray.rs:Tray::set_permission_warning`/`clear_permission_warning`)

## Settings-Fenster

- **REQ-ONBOARD-018**: Das System soll das Settings-Fenster über vier Tabs
  gliedern: Allgemein, Vokabular, Kontext, Anzeige. (`src/ui.rs:Tab`,
  `src/ui.rs:SettingsApp::tab_bar`)
- **REQ-ONBOARD-019**: WENN der Nutzer im Tray-Menü „Einstellungen…" wählt,
  soll das System das Fenster sichtbar machen und fokussieren.
  (`src/ui.rs:SettingsApp::logic` — MenuEvent `settings_id`)
- **REQ-ONBOARD-020**: WENN der Nutzer das Settings-Fenster über den
  Fenster-Schließen-Knopf schließt, soll das System das Schließen abfangen und
  das Fenster nur verstecken — talker läuft als Menüleisten-App im
  Hintergrund weiter. (`src/ui.rs:SettingsApp::logic` — `CancelClose` +
  `ViewportCommand::Visible(false)`)
- **REQ-ONBOARD-021**: WENN der Nutzer im Tray-Menü „Beenden" wählt, soll das
  System das Schließen NICHT abfangen, sondern den Prozess tatsächlich
  beenden — im Unterschied zum bloßen Fenster-Schließen (REQ-ONBOARD-020).
  (`src/ui.rs:SettingsApp::logic` — `quitting = true` + `ViewportCommand::Close`,
  Guard `!self.quitting`, Ticket-0008 Regression „Beenden wurde vom
  Fenster-verstecken-Handler verschluckt")
- **REQ-ONBOARD-022**: WENN das Fenster durch „Beenden" geschlossen wird
  während die Live-Vorschau aktiv ist, soll das System die Vorschau beenden,
  bevor der Prozess terminiert (kein verwaister Vorschau-Zustand).
  (`src/ui.rs:SettingsApp::logic` — `set_preview(false)`)

## Login-Item

- **REQ-ONBOARD-023**: Das System soll den Login-Item-Status über
  `SMAppService` als Source of Truth abfragen, nicht über eine eigene
  Config-Kopie. (`src/login_item.rs:status`)
- **REQ-ONBOARD-024**: FALLS talker nicht aus einem installierten
  `.app`-Bundle läuft (z. B. `cargo run`), DANN soll das System den
  Login-Item-Status als `Unavailable` melden und in den Settings statt der
  Checkbox den Hinweis „nur aus installiertem talker.app" anzeigen — kein
  Toggle-Versuch. (`src/login_item.rs:in_app_bundle`,
  `src/ui.rs:SettingsApp::settings_section` — Login-Item-Zeile)
- **REQ-ONBOARD-025**: WENN der Nutzer die „Beim Login starten"-Checkbox in
  den Settings umschaltet, soll das System `SMAppService` registrieren bzw.
  deregistrieren und das Ergebnis (Erfolg/Fehlermeldung) als Speicher-Hinweis
  anzeigen. (`src/ui.rs:SettingsApp::settings_section` — Login-Item-Zeile,
  `src/login_item.rs:set_enabled`)
- **REQ-ONBOARD-026**: Das System soll `SMAppService`-Auf- oder -Abbau nur
  auslösen, wenn der gewünschte Zustand vom aktuellen abweicht (idempotentes
  Toggle, kein unnötiger API-Call). (`src/login_item.rs:needs_change`, Test
  `toggle_only_acts_on_state_difference`)
- **REQ-ONBOARD-027**: FALLS der Login-Item-Status `RequiresApproval` ist
  (macOS verlangt Nutzer-Bestätigung), DANN soll das System zusätzlich zur
  aktivierten Checkbox den Hinweis „in Systemeinstellungen → Anmeldeobjekte
  bestätigen" anzeigen. (`src/ui.rs:SettingsApp::settings_section` — Login-Item-Zeile)

---

Requirements-Anzahl: 27 (`REQ-ONBOARD-001` … `REQ-ONBOARD-027`).
