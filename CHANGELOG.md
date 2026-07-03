# Changelog

Alle nennenswerten Änderungen an talker werden hier dokumentiert.

Format: [Keep a Changelog 1.1.0](https://keepachangelog.com/de/1.1.0/) ·
Versionierung: [SemVer 2.0.0](https://semver.org/lang/de/) — Policies siehe
`docs/stability.md`.

## [Unreleased]

### Fixed

- Schlägt das Einfügen fehl (Clipboard-Schreiben oder Cmd+V), wird das
  Nutzer-Clipboard trotzdem wiederhergestellt — vorher ging der ursprüngliche
  Inhalt verloren.
- Interne Fehler (Config-Lock, Tray-Icon) werden sichtbar gemeldet bzw.
  geloggt statt still verschluckt.

### Added

- Modell-Downloader (Kern): talker kann Parakeet (sherpa-onnx-Release) und
  gemma (ggml-org GGUF) selbst laden — SHA-256-verifiziert gegen gepinnte
  Hashes, mit Fortschritt, Retry bei Fehler/Checksum-Mismatch und
  Live-Freischaltung der Nicht-Roh-Modi, sobald gemma bereit ist. Downloads
  passieren nur nach Lizenz-Zustimmung (neues Config-Feld
  `model_download_consent`, Default aus). Die Erst-Start-UI dazu folgt.
- Kontext-Awareness (opt-in, Default aus): talker wählt den Cleanup-Modus
  automatisch je fokussierter App (Regeln bundle-id → Modus in der Config,
  `context_aware_enabled` + `context_rules`); ohne Regel-Treffer gilt der
  manuell gewählte Modus. Das Tray-Badge zeigt während der Aufnahme den
  aufgelösten Modus. Einstellbar im neuen Settings-Tab „Kontext":
  Feature-Schalter, Regel-Liste mit Modus-Dropdown und App-Picker über die
  laufenden Apps (Klarname, bundle-id wird automatisch erfasst).
- Lizenz: Code steht unter MIT (`LICENSE`), Abhängigkeits-Lizenzen in
  `THIRD-PARTY.html`, Modell-Lizenzlage dokumentiert in `docs/licensing.md`.
- Community-Files: CONTRIBUTING, CODE_OF_CONDUCT (Contributor Covenant 2.1),
  SECURITY (privater Meldeweg) und GitHub-Issue-/PR-Templates; README mit
  Feature-Überblick und Quickstart.
- Push-to-talk-Diktat systemweit: globale PTT-Taste (Default Fn/🌐) halten,
  sprechen, loslassen — Text landet in der fokussierten App (Clipboard + Cmd+V,
  Nutzer-Clipboard wird gesichert und wiederhergestellt).
- Lokale Spracherkennung: Parakeet TDT 0.6b v3 (de/en) via sherpa-onnx,
  vollständig on-device.
- LLM-Cleanup mit Stil-Profilen `Roh`, `Geschäftlich`, `Natürlich`,
  `LLM-optimiert` (gemma4:e2b, eingebettetes llama.cpp); ausfallsicher mit
  Fallback auf den Raw Transcript.
- Deterministische Vokabular-Korrektur per Kölner Phonetik (eigene Begriffe
  wie »Claude CLI« werden vor dem Cleanup korrigiert), abschaltbar.
- Menüleisten-App mit Modus-Schnellwechsel, Settings-Fenster (egui):
  Hotkey, Mikrofon, STT-Modellpfad, Vokabular, Overlay-Optik.
- Aufnahme-Overlay mit Live-Pegel-Wellen; Position, Breite, Farben, Trail
  konfigurierbar.
- Permission-Onboarding (Accessibility, Mikrofon) beim ersten Start;
  Event-Tap-Installation wird bei fehlender Permission automatisch nachgeholt.
- Konfiguration als TOML unter `~/Library/Application Support/talker/`;
  Legacy-Feld `cleanup_enabled` wird beim Laden zu `cleanup_mode` migriert.
