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
- **Lokale Spracherkennung** (de/en): Parakeet TDT 0.6b v3 via sherpa-onnx.
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
   - **Netzwerk beim ersten Build**: `sherpa-rs` lädt vorgebaute onnxruntime-/sherpa-Dylibs.
2. **Modelle laden** nach `~/Library/Application Support/talker/models/`
   (talker bundlet keine Modelle; sie stehen unter eigenen Lizenz-Terms —
   siehe [`docs/licensing.md`](docs/licensing.md)):
   - `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/` — [Download](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2) (entpacken)
   - `gemma-4-E2B-it-Q8_0.gguf` — von `ggml-org/gemma-4-E2B-it-GGUF` (Hugging Face)
3. **Bauen & installieren:** `make install` (s. o.).
4. **Permissions erteilen** (einmalig, siehe unten) — dann PTT-Taste halten
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

Das Bundle ist **ad-hoc signiert** (kein Developer-Account, keine Notarisierung)
und nur für den eigenen Mac gedacht. Beim ersten Start:

1. **Bedienungshilfen** erlauben (Systemeinstellungen → Datenschutz & Sicherheit
   → Bedienungshilfen → talker) — für den globalen Hotkey und das Einfügen.
2. **Mikrofon** erlauben — macOS fragt beim ersten Diktat.
3. Optional in den Einstellungen (Menüleiste → Einstellungen…):
   „Beim Login starten" aktivieren.

Hinweis: Nach dem Erteilen der Bedienungshilfen-Permission talker neu starten.
Für Accessibility gibt es keinen Info.plist-Usage-String (macOS kennt dafür
keinen Key); der Hinweis kommt aus dem App-Onboarding.

## Konfiguration

`~/Library/Application Support/talker/config.toml` — wird von den Einstellungen
geschrieben (PTT-Taste, Mikrofon, Cleanup an/aus, STT-Modellpfad).

## Lizenz & Sicherheit

Code: [MIT](LICENSE) © 2026 René Leban. Abhängigkeiten: `THIRD-PARTY.html`.
Die Modelle sind **nicht** Teil dieses Repos und stehen unter eigenen Terms
(Gemma Terms of Use bzw. CC-BY-4.0) — Details in
[`docs/licensing.md`](docs/licensing.md).

Sicherheitslücken bitte privat melden — siehe [`SECURITY.md`](SECURITY.md).
