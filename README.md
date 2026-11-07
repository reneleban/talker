<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="resources/brand/talker-logo-dark.svg">
    <img src="resources/brand/talker-logo-light.svg" alt="talker" width="300">
  </picture>
</p>

<p align="center">
  <b>Privacy-first Diktat für macOS — Push-to-talk, 100 % on-device, MIT.</b><br>
  Taste halten, sprechen, loslassen: der fertige Text landet in der fokussierten App.
  Kein Audio, kein Text verlässt den Rechner (<a href="docs/security-audit.md">Nachweis</a>).
</p>

<!-- TODO: Demo-GIF (Diktat in eine beliebige App) -->

## Voice-to-Prompt: Diktat für AI-Coding-CLIs

Das Alleinstellungsmerkmal neben Open Source: der Cleanup-Modus
**LLM-optimiert** macht aus gesprochenem Rambling einen fertigen,
strukturierten Prompt für Claude Code, Codex & Co. — statt Gedanken
mühsam in Prompt-Form zu tippen, sprichst du sie einfach:

> *gesprochen:* „ähm also die config, wenn da ein feld fehlt soll er nicht
> crashen sondern ähm defaults nehmen, und schreib da bitte auch n test
> für"
>
> *eingefügt:* „Ergänze robustes Config-Parsing: fehlende Felder dürfen
> nicht crashen, sondern fallen auf Defaults zurück. Schreibe einen Test
> dafür."

Alles lokal — auch dein Prompt-Rohmaterial geht an keine Cloud.

## Features

- **Push-to-talk systemweit**: globale Taste (Default Fn/🌐) halten und
  sprechen — funktioniert in jeder App, die Texteingabe annimmt.
- **Lokale Spracherkennung** (de/en): NVIDIA Nemotron 3.5 ASR Streaming 0.6b via sherpa-onnx.
- **Cleanup-Modi** (gemma4:e2b, eingebettetes llama.cpp — kein Server, kein
  Daemon): `Roh` (Transkript unverändert), `Geschäftlich` (formal, entfernt
  unsichere Floskeln), `Natürlich` (nur Füllwörter/Interpunktion, dein Ton
  bleibt), `LLM-optimiert` (Voice-to-Prompt, s. o.). Ausfallsicher: fällt
  das LLM aus, wird das rohe Transkript eingefügt.
- **Eigenes Vokabular**: deterministische Korrektur per Kölner Phonetik
  (z. B. »Claude CLI«), abschaltbar.
- **Siri-Style-Overlay**: Leuchtspur-Wellen zeigen Aufnahme und Pegel;
  Farben/Tempo konfigurierbar.
- **Robust**: Nutzer-Clipboard wird gesichert und wiederhergestellt; keine
  Pipeline-Stufe bricht hart ab.
- Menüleisten-App mit Einstellungen (PTT-Taste, Mikrofon, Cleanup-Modus,
  Login-Start).

## Source-first: bau es selbst

Es gibt bewusst **kein fertiges Binary**. Du baust talker aus dem Quellcode —
und weißt damit exakt, was auf deinem Mikrofon lauscht: jede Zeile ist
auditierbar (MIT), der Egress-Nachweis steht in
[`docs/security-audit.md`](docs/security-audit.md). Der Build ist ein
Einzeiler:

```sh
make install  # baut talker.app und installiert nach ~/Applications
```

## Quickstart

1. **Voraussetzungen** (frischer Mac):
   - macOS 13+, Apple Silicon.
   - **Xcode Command Line Tools**: `xcode-select --install` (liefert `clang`, `git`).
   - **Rust via rustup**: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` —
     installiert `cargo`. `rust-toolchain.toml` zieht dann automatisch die gepinnte
     Version (1.94.1) inkl. `rustfmt`/`clippy`.
   - **cmake**: `brew install cmake` — baut das eingebettete llama.cpp (`llama-cpp-2`).
   - **Netzwerk beim ersten Build**: `sherpa-onnx-sys` lädt vorgebaute, statisch gelinkte onnxruntime-/sherpa-Bibliotheken.
2. **Modelle laden** nach `~/Library/Application Support/talker/models/`
   (talker bundlet keine Modelle; sie stehen unter eigenen Lizenz-Terms —
   siehe [`docs/licensing.md`](docs/licensing.md)):
   - `sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/` — [Download](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2) (entpacken; talker lädt es beim Erst-Start auch selbst)
   - `gemma-4-E2B-it-Q8_0.gguf` — von `ggml-org/gemma-4-E2B-it-GGUF` (Hugging Face)
3. **Signatur-Zertifikat anlegen (einmalig):** `make cert` — sonst verfallen
   die Permissions bei jedem Update (siehe Known Issue unten).
4. **Bauen & installieren:** `make install` (s. o.).
5. **Permissions erteilen** (einmalig, siehe unten) — dann PTT-Taste halten
   und lossprechen.

## Entwicklung

```sh
make verify   # Build + Tests + Clippy (vor jedem Commit)
make run      # startet als Menüleisten-App aus dem Quellbaum
make stress   # Cleanup-Qualitäts-Testreihe
make help     # alle Kommandos
```

Ein pre-commit-Hook (cargo-husky, installiert sich beim ersten `cargo test`)
erzwingt fmt, Clippy und die Unit-Tests — Details in
[`CONTRIBUTING.md`](CONTRIBUTING.md). Contributions willkommen.
Architektur & Entscheidungen: `docs/adr/`, Begriffe: `CONTEXT.md`.

## App-Bundle & Permissions (Personal Use)

Wichtig: Das Bundle wird ersetzt, nie überkopiert — In-Place-Kopieren macht
die Code-Signatur aus Kernel-Sicht ungültig (App startet nicht mehr).

Das Bundle ist lokal signiert (kein Developer-Account, keine Notarisierung —
bewusst: source-first, siehe oben) und nur für den eigenen Mac gedacht.
**Vor dem ersten `make install` einmalig `make cert` ausführen** — das legt ein
selbstsigniertes Codesign-Zertifikat `talker-dev` im Login-Schlüsselbund an.
Beim ersten Start:

1. **Bedienungshilfen** erlauben (Systemeinstellungen → Datenschutz & Sicherheit
   → Bedienungshilfen → talker) — für den globalen Hotkey und das Einfügen.
2. **Mikrofon** erlauben — macOS fragt beim ersten Diktat.
3. Optional in den Einstellungen (Menüleiste → Einstellungen…):
   „Beim Login starten" aktivieren.

Hinweis: Nach dem Erteilen der Bedienungshilfen-Permission talker neu starten.
Für Accessibility gibt es keinen Info.plist-Usage-String (macOS kennt dafür
keinen Key); der Hinweis kommt aus dem App-Onboarding.

### Known Issue: Permissions nach Update wieder weg

macOS bindet die erteilten Rechte (Bedienungshilfen, Mikrofon) an die
Code-Signatur der App. Ohne das `talker-dev`-Zertifikat signiert der Build
**ad-hoc** — dann ist talker für macOS nach jedem `make install` eine „neue"
App und alle Rechte sind weg (der Eintrag in den Systemeinstellungen wirkt
gesetzt, ist aber tot: einmal löschen und neu erteilen).

**Workaround (einmalig pro Mac):** `make cert`, danach `make install`. Nach
diesem einen Wechsel der Signatur die Rechte ein letztes Mal neu erteilen —
ab dann überleben sie jedes Update, weil Bundle-ID und Zertifikat stabil
bleiben. Alternative von Hand: Schlüsselbundverwaltung → Zertifikatsassistent →
„Ein Zertifikat erstellen…" → Name `talker-dev`, Typ *Codesignierung*.

## Konfiguration

`~/Library/Application Support/talker/config.toml` — wird von den Einstellungen
geschrieben (PTT-Taste, Mikrofon, Cleanup an/aus, STT-Modellpfad, Protokoll-Detailgrad).

## Diagnose: Logdatei, Messzeile, Mitschnitt

**Logdatei.** talker schreibt alle Meldungen nach `~/Library/Logs/talker/talker.log` —
egal ob Start aus dem Terminal, dem Finder oder als Login-Item. Wird die Datei größer
als 1 MB, rotiert talker: `talker.1.log` … `talker.4.log`, die älteste Datei wird
gelöscht (zusammen höchstens 5 MB, nie nach Alter). Lässt sich das Verzeichnis nicht
anlegen, läuft talker ohne Datei weiter und schreibt nach stderr. In den Settings öffnet
„Log im Finder zeigen" die Datei im Finder.

**Detailgrad.** `log_level` in der `config.toml` (`error`, `warn`, `info`, `debug`; Default
`info`), in den Settings unter „Protokoll" umschaltbar — ohne Neustart. Die Umgebungsvariable
`TALKER_LOG=<grad>` übersteuert den Wert für den Prozess; die Settings zeigen dann „durch
TALKER_LOG übersteuert". Ein ungültiger Wert wird mit einer Warnung ignoriert. Auf `info`
steht pro Diktat genau eine Zeile (die Messzeile), nichts pro Block oder Segment; auf `debug`
kommen Details dazu. Transcript-Text steht nie im Log.

**Messzeile.** Nach jedem Diktat, auch bei verworfenen oder fehlgeschlagenen:

```
Diktat #17: Taste→Mikro bereit 120 ms · →erster Block 380 ms · →erstes Signal 640 ms · gehalten 4,2 s · aufgenommen 3,6 s · Loslassen→Text 780 ms (STT 520 ms · Cleanup 240 ms) · →eingefügt 35 ms · Gerät: AirPods (48 kHz, 1 Kanal) · Einfügen: Am Ende · Modus: Natürlich · Kaltstart: nein · Ergebnis: eingefügt
```

Nicht erreichte Messpunkte stehen als `—`. Satzweise/Wortgruppenweise ergänzen
`→erstes Segment` und `Segmente`; wurde ein Modell geladen, steht `Modell-Laden …` dabei.

**Debug-Mitschnitt.** Nur mit gesetzter Umgebungsvariable `TALKER_DEBUG_WAV_DIR` schreibt talker
je Diktat `<nr>-mic.wav` (rohe Mikrofon-Blöcke im Geräteformat) und `<nr>-model.wav` (exakt der
Modell-Eingang, 16 kHz mono) in dieses Verzeichnis (32-bit Float; `<nr>` = Nummer der Messzeile).
Das Verzeichnis wird (0700) angelegt, die Dateien sind nur für dich lesbar (0600). Dateien werden nie
gelöscht oder überschrieben: Gibt es `<nr>-mic.wav` schon (die Nummer beginnt bei jedem App-Start
wieder bei 1), warnt talker im Log und schreibt sie nicht — nimm ein leeres Verzeichnis je Sitzung.
Bei aktivem Mitschnitt steht beim Start eine Warnung im Log. Ohne die Variable schreibt talker
kein Audio auf die Platte. Beim Bundle über `open` starten:

```sh
open --env TALKER_DEBUG_WAV_DIR="$HOME/talker-debug" ~/Applications/talker.app
```

**Messreihe.** `make latency-eval` verarbeitet alle STT-Fixtures je Am Ende/Satzweise ×
Roh/Natürlich ohne Einfügen und gibt eine Markdown-Tabelle mit p50/p95 aus. `N=<zahl>` setzt
die Läufe je Fall (Default 10, dazu ein verworfener Aufwärmlauf), `LATENCY_OUT=<datei>` schreibt
die Tabelle zusätzlich in eine Datei. `N=3` ist der Schnelllauf; `N=10` dauert über 30 Minuten,
weil Satzweise im Echtzeittakt läuft. Die Spalte „je Audio-s“ setzt `Loslassen→Text` ins
Verhältnis zur Clip-Länge (die Latenz am Ende wächst mit ihr). Braucht das Nemotron-Modell;
ohne Gemma laufen nur die Roh-Fälle, die Natürlich-Fälle stehen unter „Übersprungen“.

**Fixtures.** Die zehn FLEURS-Clips (`tests/fixtures/stt/fleurs-de-*`, CC-BY-4.0, siehe
`tests/fixtures/stt/ATTRIBUTION.md`) erzeugt `scripts/fetch_fleurs_fixtures.sh` neu; das
Skript lädt sie einmalig aus Hugging Face (Netz nötig).

## Lizenz & Sicherheit

Code: [MIT](LICENSE) © 2026 René Leban. Abhängigkeiten: `THIRD-PARTY.html`.
Die Modelle sind **nicht** Teil dieses Repos und stehen unter eigenen Terms
(Gemma Terms of Use bzw. CC-BY-4.0) — Details in
[`docs/licensing.md`](docs/licensing.md).

Sicherheitslücken bitte privat melden — siehe [`SECURITY.md`](SECURITY.md).
