# talker — Engineering-Regeln (Repo)

Verbindlich für alle, die in diesem Repo Code schreiben (Menschen wie Agenten/Fable).
Kontext: Entscheidungen in `docs/adr/`, Begriffe in `CONTEXT.md`.

## Stabilitäts-Policies (verbindlich)
- **SemVer, CHANGELOG (Keep a Changelog), Config-Versionierung/Deprecation** —
  Details in `docs/stability.md`. Kurzfassung: Config-TOML ist ein
  Kompatibilitäts-Vertrag (4 Stellen in `config.rs` pro Feld, Deprecation statt
  stillem Break); jede nutzersichtbare Änderung bekommt einen CHANGELOG-Eintrag
  im selben PR; Breaking Changes werden explizit markiert.

## Scope & Stack (hart)
- **Nur macOS, aktuellste Version.** Keine Cross-Platform-Abstraktionen — native macOS-APIs direkt.
- **Rust.** ML-Engines (STT/LLM) via FFI, gekapselt hinter Traits (`Transcriber`, `LlmCleaner`) — ADR-0001.

## Test-Driven & Testabdeckung
- **TDD wo möglich** (red-green-refactor): erst der Test, der die AK ausdrückt, dann Code.
- **Tests-pro-Story ist Definition of Done.** Jede Akzeptanzkriterium hat mind. einen Test — AK ↔ Test 1:1. Ohne Tests ist eine Story nicht fertig.
- **Grenzfälle sind Pflicht:** leere Eingaben, Min/Max, ein-über/-unter-Grenze, Fehlerpfade. Out-of-Scope-Items als „darf nicht passieren"-Tests wo sinnvoll.
- **Kritische Logik voll abgedeckt:** Clipboard save/restore, Resampling, TOML load/save, Cleanup-Fallback. Kein starres Prozent-Gate (vermeidet Test-Theater für GUI/FFI-Glue).

## Clean Code
- **YAGNI strikt:** nur bauen, was die aktuelle AK fordert. Keine Stubs „für später", keine ungenutzten Felder/Methoden, keine TODO-Platzhalter.
- Bestehende Dateien editieren statt neue anlegen. Minimale, fokussierte Changes — kein Refactor über den Auftrag hinaus.
- Keine voreiligen Abstraktionen — drei ähnliche Zeilen > ein einmal genutzter Helper.
- Validieren nur an Systemgrenzen (User-Input, externe APIs/FFI).
- Keine Docstrings/Kommentare zu unverändertem Code.

## Fehlerbehandlung
- `Result` + `thiserror`/`anyhow`. **Keine Panics im Hot-Path** (Aufnahme → STT → Cleanup → Injection).
- Die Pipeline darf an keiner Stufe hart abbrechen: Cleanup-Fehler → Fallback auf Raw Transcript; fehlende Permission → sichtbarer Hinweis statt stillem Fail.

## Toolchain (gepinnt)
- Rust **1.94.1** (`rust-toolchain.toml`), Komponenten rustfmt + clippy. „Grün" ist damit für alle identisch definiert.
- Clippy-Lints: `[lints.clippy] all = "warn"` in `Cargo.toml`, `msrv` in `clippy.toml`.

## verify:fast (vor jedem Commit)
- `cargo build` + `cargo test` + `cargo clippy` müssen grün sein. Lokal nicht grün → nicht committen, nicht pushen.
- Feature-Stories (UI/Injection) zusätzlich einmal real benutzen (Field-Test), nicht nur type-checken.

## Coverage (Report, kein Gate)
- Sichtbarkeit via `cargo llvm-cov` (einmalig installieren: `cargo install cargo-llvm-cov`). Berichtet die Zahl, blockiert nichts.
- Verbindlich bleibt die AK-basierte Abdeckung oben, nicht ein Prozentwert.

## Git
- **Immer Feature-Branch**, nie direkt auf `main`. Naming: `feat/<ticket>-<kurzname>`, `fix/...`.
- **Conventional Commits**: `type(scope): beschreibung`.
- Kein `--no-verify`, keine Hooks überspringen. Merge nach `main` nur nach manuellem Review/OK.

## Stopp-Punkte (an den Menschen übergeben, nicht raten)
- macOS-Permissions (Accessibility, Mikrofon) kann kein Prozess sich selbst geben — anhalten, erklären, auf OK warten.
- Nach 2 erfolglosen Versuchen an derselben Stelle: stoppen, Root-Cause + Optionen melden.
