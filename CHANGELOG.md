# Changelog

Alle nennenswerten Änderungen an talker werden hier dokumentiert.

Format: [Keep a Changelog 1.1.0](https://keepachangelog.com/de/1.1.0/) ·
Versionierung: [SemVer 2.0.0](https://semver.org/lang/de/) — Policies siehe
`docs/stability.md`.

## [Unreleased]

### Added

- **Einstellung „Sprache“:** In den Settings wählst du Automatisch, Deutsch oder
  Englisch; die Wahl gilt ab dem nächsten Diktat ohne Neustart, im Am-Ende- wie im
  Live-Modus. Neues Config-Feld `language` (`"auto"`, `"de"`, `"en"`, Default `"auto"`);
  bestehende Configs bleiben unverändert gültig, ein unbekannter Wert wird wie ein
  ungültiger `insert_mode` behandelt (Hinweis, Defaults, Datei unverändert). Die
  Messzeile nennt die Sprache (`· Sprache: …`) direkt hinter dem Modus.
- `make latency-eval LANGUAGE=auto|de|en` misst mit der gewählten Sprache (Default
  `auto`, die Tabelle nennt sie); ein ungültiger Wert bricht mit Hinweis ab.
- Fünf weitere FLEURS-Testclips von Sprecherinnen (CC-BY-4.0) neben den zehn
  bisherigen; `scripts/fetch_fleurs_fixtures.sh` erzeugt sie reproduzierbar.
- **Logdatei mit Rotation und einstellbarem Detailgrad:** talker schreibt nach
  `~/Library/Logs/talker/talker.log` (Start aus Terminal, Finder oder als Login-Item);
  ab 1 MB wird rotiert, höchstens 4 ältere Dateien bleiben (zusammen ≤ 5 MB). In den
  Settings wählst du unter „Protokoll“ Fehler/Warnungen/Info/Debug (wirkt sofort) und
  öffnest die Datei mit „Log im Finder zeigen“. Neues Config-Feld `log_level`
  (`"error"`, `"warn"`, `"info"`, `"debug"`, Default `"info"`); bestehende Configs
  bleiben unverändert gültig. Die Umgebungsvariable `TALKER_LOG` übersteuert den
  Grad für den Prozess.
- **Messzeile je Diktat:** Nach jedem Diktat (auch verworfen oder fehlgeschlagen) steht
  genau eine Zeile im Log mit Taste→Mikro bereit, →erster Block, →erstes Signal,
  gehalten, aufgenommen, Loslassen→Text (STT, Cleanup), →eingefügt, Gerät, Modus,
  Kaltstart und Ergebnis. Kein Transcript-Text im Log.
- **Debug-Mitschnitt:** Mit `TALKER_DEBUG_WAV_DIR` schreibt talker je Diktat
  `<nr>-mic.wav` und `<nr>-model.wav` (opt-in; ohne die Variable kein Audio auf der
  Platte). Beim Bundle: `open --env TALKER_DEBUG_WAV_DIR=… talker.app`.
- **`make latency-eval`:** Latenz-Messreihe über alle STT-Fixtures (p50/p95, WER,
  Kaltlade-Dauer); dazu zehn FLEURS-Testclips (CC-BY-4.0) und eine Regression
  „erstes Wort“, die den bekannten Verlust der ersten Wörter reproduziert.
- **Mikrofon-Start-Anzeige:** Beim Drücken der PTT-Taste erscheint sofort ein
  Balken „Mikrofon startet“; die Wellen zeigt das Overlay erst, wenn das Mikrofon
  wirklich Audio liefert. Der Start läuft im Hintergrund und blockiert die Taste
  nicht mehr. Lässt du vor dem Start los, wird die Aufnahme abgebrochen; startet
  das Mikrofon nach 5 s nicht, erscheint eine Fehlermeldung. Hinweis: Ein
  fehlgeschlagener Mikrofon-Start zeigt jetzt die Overlay-Meldung „Mikrofon nicht
  verfügbar“ statt der Tray-Warnung.
- **Live-Diktat** (ADR-0009): In den Settings wählbares Einfügen — *Am Ende*
  (wie bisher, Default), *Satzweise* (jeder Satz nach einer Sprechpause) oder
  *Wortgruppenweise* (laufend, sobald Wörter feststehen). Der Cleanup-Modus
  richtet sich danach: bei *Satzweise* sind Roh, Geschäftlich und Natürlich
  erlaubt, bei *Wortgruppenweise* nur Roh; LLM-optimiert gibt es nur bei *Am Ende*.
  Neue Config-Felder `insert_mode` (`"end"`, `"sentence"`, `"phrase"`) und
  `endpoint_silence_ms` (Sprechpause für *Satzweise*, Default 700, wirkt nach
  Neustart); bestehende Configs bleiben unverändert gültig. Bereits eingefügter
  Text wird nicht zurückgenommen.

### Changed

- Der Mikrofon-Start nutzt das gemerkte Gerät samt Format und zählt die Geräte nicht
  bei jedem Tastendruck neu auf; ein Wechsel von `mic_device`, des Standardgeräts
  oder des Formats wird erkannt. Scheitert der Start, versucht talker genau einmal
  mit neu ermitteltem Gerät, dann folgt der sichtbare Fehler. Zwischen zwei
  Aufnahmen bleibt kein Stream offen, kein Gerät wird exklusiv belegt.

- Logs gehen in eine Datei statt auf stderr (beim Start aus dem Terminal erscheinen
  sie weiter dort, wenn die Datei nicht angelegt werden kann). Der Wortlaut der
  Logzeilen ist kein stabiler Vertrag.
- **Breaking:** Spracherkennung wechselt von Parakeet TDT 0.6b v3 auf NVIDIA
  Nemotron 3.5 ASR Streaming 0.6b (ADR-0008) — ein Modell für alle Modi, Grundlage
  für Live-Diktat. Erst-Start-Download des neuen Modells (ca. 475 MB), danach läuft
  alles lokal. Migration: beim ersten Start nach dem Update erscheint der
  Lizenz-Dialog erneut (neue Lizenz: OpenMDW-1.1, neues Config-Feld
  `stt_license_consent`, Default `false`); ein `stt_model_dir`, das auf das alte
  Parakeet-Verzeichnis zeigt, wird automatisch auf das neue Verzeichnis
  umgestellt. Das alte Modell bleibt auf der Platte (`sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`)
  und kann gelöscht werden. Preis des Wechsels laut `make stt-eval` (24 TTS-Fixtures):
  die Wartezeit nach dem Loslassen steigt von ca. 0,35 s auf ca. 0,7–1,0 s,
  die Wortfehlerrate von 15,0 % auf 17,3 %. Dafür liefert das Modell
  Interpunktion und Groß-/Kleinschreibung selbst.
- Spracherkennung läuft jetzt über den offiziellen Crate `sherpa-onnx` 2.0.0
  statt über das archivierte `sherpa-rs`. Modell (Parakeet TDT 0.6b v3), Trait
  `Transcriber` und Config bleiben unverändert; einzelne Sätze werden durch die
  neuere eingebettete Laufzeit minimal anders erkannt (24-Fälle-Testreihe:
  mittlere WER 15,0 % statt 16,0 %, Begriff-Treffer unverändert 19/29). Die App
  bringt keine losen Dylibs mehr mit (statisch gelinkt, Bundle ohne `Frameworks/`).
  Grundlage für spätere Streaming-Modelle, siehe `docs/research/`.

### Security

- TLS-Stack aktualisiert (`rustls` 0.23.46, `rustls-webpki` 0.103.16,
  `webpki-roots` 1.0.11): behebt mehrere RUSTSEC-Advisories (u. a. RUSTSEC-2026-0285,
  RUSTSEC-2026-0346) — angenommene TLS-1.3-Handshake-Nachrichten auf der falschen
  Verschlüsselungsstufe, `Debug`-Ausgabe mit TLS-Secrets, Brotli-Dekompression mit
  bis zu 1 GiB Speicher vor der Authentifizierung, Downgrade durch unauthentifiziertes
  ServerHello sowie Vertrauen in nicht mehr vertrauenswürdige Zwischenzertifikate.
  Betrifft den Modell-Downloader (`ureq`). Nur `Cargo.lock`, kein API-Wechsel.
- `webbrowser` auf 1.2.2 aktualisiert (RUSTSEC-2026-0257): auf Unix setzte die alte
  Version die URL in das `BROWSER`-Template ein und tokenisierte erst danach — eine
  URL mit Leerzeichen konnte so zu zusätzlichen Browser-Argumenten werden
  (z. B. `--remote-debugging-port`, `--proxy-server`). Transitiv über
  eframe → egui-winit; nur `Cargo.lock`, kein API-Wechsel.

### Fixed

- Satzanfang im Live-Modus: Nach einer Sprechpause gehen die ersten Wörter des
  nächsten Satzes nicht mehr verloren. Der Recognizer läuft ohne Reset durch, die
  Segmente entstehen aus dem Textzuwachs (ADR-0015); führende Satzzeichen hängen am
  vorigen Segment statt als eigener Satz zu beginnen.
- Das Overlay zeigt die Wellen erst, wenn tatsächlich Audio ankommt (erstes Signal);
  davor bleibt der Balken. Kommt nach 5 s kein Signal, erscheint „Mikrofon antwortet
  nicht“, die Aufnahme läuft weiter.
- Vokabular-Korrektur greift nicht mehr zu hart: häufige Wörter werden nicht
  mehr fälschlich durch klanglich nur grob ähnliche Vokabular-Begriffe ersetzt
  (z. B. „kaputt" → „Kyroth", „Tunnel" → „TOML"). Der unscharfe
  Phonetik-Vergleich braucht jetzt längere Klang-Codes, und bei exakt gleichem
  Klang-Code muss zusätzlich das Schriftbild nah sein. Die gewollten
  Korrekturen (Tommel→TOML, Clotzelei→Claude CLI, Kubanetis→Kubernetes,
  Darker Image→Docker Image) bleiben erhalten. Echte Beinah-Homophone
  (Pudding↔Padding, merke↔merge) sind phonetisch nicht trennbar — solche
  Begriffe besser nicht ins Vokabular aufnehmen, das erledigt der LLM-Cleanup.
- Erst-Start-Permissions ohne manuellen Neustart: der Mikrofon-Dialog kommt
  jetzt sofort beim ersten Start (nicht erst beim ersten Diktat), und nach dem
  Erteilen der Bedienungshilfen-Berechtigung startet talker sich einmal selbst
  neu, damit der globale Hotkey ohne Zutun funktioniert (macOS cached die
  Entscheidung pro Prozess). Laufende Modell-Downloads werden dabei nicht
  abgebrochen — der Neustart wartet, bis sie fertig sind.
- Schlägt das Einfügen fehl (Clipboard-Schreiben oder Cmd+V), wird das
  Nutzer-Clipboard trotzdem wiederhergestellt — vorher ging der ursprüngliche
  Inhalt verloren.
- Interne Fehler (Config-Lock, Tray-Icon) werden sichtbar gemeldet bzw.
  geloggt statt still verschluckt.
- Cleanup-Modus Natürlich normalisiert jetzt echte Mengen-, Datums- und
  Zeitangaben zu Ziffern (z. B. „zwölftausendfünfhundert" → „12.500", auch
  kleine eingebettete Zahlen wie „zwei" → „2") — im selben Format, das der
  Modus Geschäftlich schon produziert. Ordinalzahlen und feste Redewendungen
  mit Zahlwörtern („eins zu eins", „Schritt für Schritt") bleiben unverändert
  Wort.

### Added

- `make cert`: legt einmalig das selbstsignierte Codesign-Zertifikat
  `talker-dev` an, damit die macOS-Permissions (Bedienungshilfen, Mikrofon)
  Updates überleben — ohne Zertifikat signiert der Build ad-hoc und macOS
  behandelt jede neue Installation als neue App (Known Issue im README).
- Modell-Downloader: talker lädt Parakeet (sherpa-onnx-Release) und gemma
  (ggml-org GGUF) beim Erst-Start selbst — SHA-256-verifiziert gegen gepinnte
  Hashes, mit Fortschritt und Retry bei Fehler/Checksum-Mismatch. Downloads
  starten erst nach Zustimmung zu beiden Modell-Lizenzen (einmalig im
  Erst-Start-Fenster; Config-Feld `model_download_consent`, Default aus).
  Parakeet lädt blockierend mit Fortschrittsbalken (PTT bleibt gesperrt, bei
  Druck erscheint „talker richtet sich ein … X %"; Tray zeigt ein
  durchgestrichenes Setup-Icon), gemma lädt im Hintergrund — die Nicht-Roh-Modi
  sind bis dahin ausgegraut und schalten sich ohne Neustart frei. Neuer
  Einstellungen-Bereich „Modelle" mit Status je Modell (installiert/lädt
  X %/fehlt/beschädigt) und Neu-laden/Reparieren-Button. Abgebrochene
  Downloads (Netzfehler, App beendet) werden beim nächsten Versuch
  fortgesetzt statt neu geladen (HTTP-Range-Resume); die SHA-256-Prüfung
  nach Abschluss bleibt unverändert.
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
