# Lizenzen — Code vs. Modelle (Ticket-0021)

## Code: MIT

Der gesamte Quellcode dieses Repos steht unter der **MIT-Lizenz**
(`LICENSE` im Root, Copyright © 2026 René Leban; `license = "MIT"` in
`Cargo.toml`).

### SPDX-Header-Policy

**Keine SPDX-Header in Quelldateien.** Begründung: Single-Crate-Repo mit
genau einer Lizenz — `LICENSE` im Root plus das maschinenlesbare
`license`-Feld in `Cargo.toml` sind eindeutig; Header in jeder Datei wären
Pflege-Rauschen ohne Informationsgewinn. Wer einzelne Dateien
weiterverwendet, unterliegt der Repo-`LICENSE`. Sollte das Repo je
Mehrfach-Lizenzen mischen, wird diese Policy revidiert.

### Third-Party-Abhängigkeiten

`THIRD-PARTY.html` im Root listet alle Abhängigkeiten des macOS-Builds mit
Lizenztexten. Automatisiert generiert — bei Dependency-Änderungen neu erzeugen:

```sh
cargo about generate about.hbs -o THIRD-PARTY.html
```

Akzeptierte Lizenzen: `about.toml` (konsistent zu `deny.toml`, das als Gate
via `cargo deny check` läuft — siehe `docs/security-audit.md`).

## Modelle: eigene Terms, NICHT MIT

talker **bundlet keine Modelle** und lädt sie auch nicht automatisch nach —
der Nutzer lädt sie selbst (README). Damit redistribuieren wir nichts; die
Lizenzpflichten liegen beim Nutzer und gelten ab Download:

| Modell | Quelle | Lizenz |
|---|---|---|
| gemma4:e2b (GGUF) | `ggml-org/gemma-4-E2B-it-GGUF` (Hugging Face) | [Google Gemma Terms of Use](https://ai.google.dev/gemma/terms) — kein OSS; Nutzungsbedingungen inkl. Prohibited-Use-Policy akzeptiert der Nutzer beim Download |
| Nemotron 3.5 ASR Streaming 0.6b (int8, sherpa-onnx-Konvertierung) | k2-fsa/sherpa-onnx Releases, Original `nvidia/nemotron-3.5-asr-streaming-0.6b` | OpenMDW-1.1 laut [Model Card](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b) („ready for commercial use"); Lizenztext und Pflichten vor einer Weitergabe prüfen |

Hinweis: Bis zum Modellwechsel (ADR-0008) nutzte talker Parakeet TDT 0.6b v3
(CC-BY-4.0). Die OpenMDW-1.1-Angabe für Nemotron stammt aus der Model Card
(2026-10-25); der Lizenztext selbst wurde nicht im Volltext geprüft.

**Kopplung an einen künftigen App-Downloader:** sollte talker je Modelle
selbst herunterladen, muss der Download-Flow die jeweiligen Terms anzeigen
und die Zustimmung des Nutzers einholen, bevor geladen wird (insbesondere
Gemma Terms of Use).
