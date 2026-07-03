# Modell-Downloader — Anforderungen (EARS)

Feature-Bereich: Erst-Start-Download von STT-Modell (Parakeet, blockierend)
und Cleanup-LLM (gemma4:e2b, Hintergrund) inkl. Lizenz-Consent,
SHA-256-Verifikation und Resume abgebrochener Downloads.

Muster/ID-Konvention: siehe [`README.md`](README.md). Begriffe: `CONTEXT.md`.

## Lizenz-Consent

- **REQ-MODEL-001**: WÄHREND das Consent-Flag `model_download_consent` nicht
  gesetzt ist, soll das System für Parakeet und gemma den Zustand
  `ConsentPending` halten und keinen Download starten
  (`src/models.rs:initial_state`, `run_download`; Ticket-0028 AK 3/6).
- **REQ-MODEL-002**: FALLS `run_download` ohne gesetztes Consent-Flag
  aufgerufen wird, DANN soll das System den Zustand auf `ConsentPending`
  zurücksetzen und den Download mit einem Fehler verweigern
  (`src/models.rs:run_download`; Ticket-0028 AK 6).
- **REQ-MODEL-003**: WENN der Nutzer im Erst-Start-Fenster „Lizenzen
  akzeptieren und Modelle laden" bestätigt, soll das System das
  Consent-Flag in der Config persistieren und anschließend die Downloads
  für Parakeet und gemma anstoßen (`src/ui.rs:setup_view`
  `SetupStage::Consent`; Ticket-0029 AK 1).
- **REQ-MODEL-004**: Das System soll im Consent-Schritt beide Lizenzen
  (CC-BY-4.0 für Parakeet/NVIDIA, Google Gemma Terms of Use inkl.
  Prohibited-Use-Policy) mit Verweis-Link anzeigen, bevor ein Download
  startet (`src/ui.rs:setup_view` `SetupStage::Consent`; Ticket-0029 AK 1,
  `docs/licensing.md`).

## Download & Verifikation

- **REQ-MODEL-005**: Das System soll den Präsenz-/Integritäts-Zustand
  eines Modells über SHA-256-Vergleich gegen gepinnte Hashes ermitteln:
  keine Datei vorhanden → `Missing`, alle Dateien vorhanden mit korrektem
  Hash → `Ready`, sonst → `Corrupt` (`src/models.rs:check_files`,
  `check`; Ticket-0028 AK 1).
- **REQ-MODEL-006**: WÄHREND Parakeet nicht im Zustand `Ready` ist, soll
  das System Push-to-talk blockieren und den Zustandstext aus
  `setup_hint()` anzeigen (`src/models.rs:setup_hint`,
  `src/pipeline.rs:press`; Ticket-0028 AK 3, Ticket-0029 AK 3).
- **REQ-MODEL-007**: WÄHREND gemma nicht im Zustand `Ready` ist, soll das
  System alle Cleanup-Modi außer `Roh` in der UI sperren
  (`src/models.rs:mode_available`, `src/ui.rs:SettingsApp::settings_section`;
  Ticket-0029 AK 4).
- **REQ-MODEL-008**: WENN gemma den Zustand `Ready` erreicht, soll das
  System die Nicht-Roh-Cleanup-Modi ohne Neustart über den geteilten
  `ModelsState` freischalten (`src/models.rs:llm_modes_available`;
  Ticket-0028 AK 4).
- **REQ-MODEL-009**: Das System soll Parakeet-Downloads blockierend im
  Vordergrund und den gemma-Download als Hintergrund-Task ausführen, ohne
  dass ein laufender gemma-Download Push-to-talk blockiert
  (`src/models.rs:spawn_background_download`, `run_download`;
  Ticket-0028 AK 3).
- **REQ-MODEL-010**: Das System soll während eines laufenden Downloads
  den Fortschritt in Prozent (`Downloading { pct }`) fortlaufend
  aktualisieren und im Setup-Fenster sowie im „Modelle"-Bereich der
  Einstellungen als Fortschrittsbalken anzeigen (`src/models.rs:percent`,
  `src/ui.rs:SettingsApp::setup_view` — `SetupStage::Downloading`,
  `src/ui.rs:SettingsApp::settings_section`; Ticket-0029 AK 1/4).
- **REQ-MODEL-011**: WÄHREND das Setup nicht abgeschlossen ist (Parakeet
  nicht `Ready`), soll das System das Tray-Icon im durchgestrichenen
  Setup-Zustand anzeigen (`src/tray.rs:set_setup`,
  `MicStyle::Setup`; Ticket-0029 AK 2).

## Resume & Fehlerbehandlung

- **REQ-MODEL-012**: FALLS ein Netzwerkfehler während des Downloads
  auftritt, DANN soll das System den Modell-Zustand auf
  `Error(<Meldung>)` setzen, den Teildownload (`.partial`) unangetastet
  liegen lassen und einen Retry über den „Reparieren"-Button erlauben
  (`src/models.rs:run_download`, `status_line`; Ticket-0028 AK 5).
- **REQ-MODEL-013**: FALLS der berechnete SHA-256-Hash einer
  abgeschlossenen Datei nicht mit dem gepinnten Hash übereinstimmt, DANN
  soll das System die Datei löschen, den Zustand auf `Corrupt` setzen und
  einen erneuten Versuch über den „Reparieren"-Button ermöglichen
  (`src/models.rs:run_download`; Ticket-0028 AK 5, Ticket-0031 AK 4).
- **REQ-MODEL-014**: WENN beim erneuten Download-Versuch ein
  Teildownload (`.partial`) existiert und der Server mit HTTP 206 auf
  eine `Range`-Anfrage antwortet, soll das System den Download ab der
  vorhandenen Dateigröße fortsetzen statt neu zu laden
  (`src/models.rs:HttpFetcher::fetch`; Ticket-0031 AK 1).
- **REQ-MODEL-015**: FALLS der Server eine `Range`-Anfrage ignoriert und
  mit HTTP 200 antwortet, DANN soll das System die Zieldatei von vorn
  überschreiben (Truncate) statt die Bytes anzuhängen
  (`src/models.rs:HttpFetcher::fetch`; Ticket-0031 AK 1).
- **REQ-MODEL-016**: FALLS der Server auf eine Resume-Anfrage mit HTTP
  416 (Range Not Satisfiable) antwortet, DANN soll das System den
  Teildownload löschen und einen Fehler melden, sodass der nächste
  Versuch neu von vorn startet (`src/models.rs:HttpFetcher::fetch`;
  Ticket-0031 AK 4).
- **REQ-MODEL-017**: Das System soll beim Fortsetzen eines Teildownloads
  die bereits vorhandenen Bytes in den Fortschritts-Callback einrechnen,
  sodass der Fortschrittsbalken beim Resume nicht bei 0 % beginnt
  (`src/models.rs:HttpFetcher::fetch`, `run_download`; Ticket-0031 AK 2).
- **REQ-MODEL-018**: WÄHREND ein Modell-Download läuft (`Downloading`
  oder `Verifying`), soll das System einen App-Neustart (Self-Relaunch)
  zurückhalten, bis der Download abgeschlossen ist
  (`src/models.rs:any_download_running`; Ticket-0031 AK 5).

## Setup-Gate & Live-Aktivierung

- **REQ-MODEL-019**: WÄHREND Parakeet nicht im Zustand `Ready` ist, soll
  das System das Erst-Start-Setup anstelle der normalen
  Einstellungs-Tabs anzeigen (`src/ui.rs:setup_stage`, `src/ui.rs:SettingsApp::ui`;
  Ticket-0029 AK 1).
- **REQ-MODEL-020**: Das System soll im „Modelle"-Bereich der
  Einstellungen je Modell den aktuellen Status
  (installiert/lädt %/fehlt/beschädigt) sowie einen kontextabhängigen
  Aktions-Button („Neu laden" bei `Missing`, „Reparieren" bei `Corrupt`
  oder `Error`) anzeigen (`src/models.rs:ModelState::status_line`,
  `src/ui.rs:SettingsApp::settings_section`; Ticket-0029 AK 4).
