# Contributing zu talker

Danke für dein Interesse! Ein paar verbindliche Konventionen halten das
Projekt wartbar.

## Scope

talker ist bewusst **nur macOS** (aktuellste Version, Apple Silicon) und
**Rust**. PRs mit Cross-Platform-Abstraktionen oder anderen Sprachen werden
nicht angenommen. Architektur-Entscheidungen stehen in `docs/adr/` — bitte
vor größeren Änderungen lesen (ebenso `CLAUDE.md` für die vollständigen
Engineering-Regeln).

## Workflow

1. **Feature-Branch**, nie direkt auf `main`:
   `feat/<kurzname>`, `fix/<kurzname>`, `docs/<kurzname>` …
2. **Conventional Commits**: `type(scope): beschreibung`
   (z. B. `fix(injection): clipboard nach fehlschlag wiederherstellen`).
3. **verify:fast vor jedem Commit** — muss grün sein, sonst nicht committen:

   ```sh
   make verify   # cargo build + cargo test + cargo clippy
   ```

   Ein **pre-commit-Hook** erzwingt das automatisch: `cargo-husky`
   (dev-dependency) installiert ihn beim ersten `cargo test` aus
   `.cargo-husky/hooks/pre-commit`. Er prüft `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings` und die Unit-Tests
   (`cargo test --lib --bins --examples`) — bewusst ohne die
   modell-schweren Integrationstests. Der Hook ist ein lokales Gate und
   ersetzt kein CI. Bewusstes Überspringen (Ausnahmefall, z. B.
   WIP auf eigenem Branch): `git commit --no-verify`.

4. PR gegen `main` öffnen. Merge nur nach Review (Squash-Merge ist Default).
5. Keine Hooks überspringen — `--no-verify` nur im dokumentierten
   Ausnahmefall (siehe oben), nie für Commits Richtung `main`.

Kein DCO-Sign-off nötig.

## Tests sind Teil der Definition of Done

- **TDD wo möglich**: erst der Test, der das Akzeptanzkriterium ausdrückt,
  dann der Code.
- Jede Story/jeder Fix bringt Tests mit — Akzeptanzkriterium ↔ Test 1:1.
- **Grenzfälle sind Pflicht**: leere Eingaben, Min/Max, ein-über/-unter-Grenze,
  Fehlerpfade.
- UI-/Feature-Änderungen zusätzlich einmal real benutzen (Field-Test), nicht
  nur type-checken.

## Code-Regeln (Kurzfassung)

- **YAGNI strikt**: nur bauen, was die aktuelle Aufgabe fordert. Keine Stubs,
  keine ungenutzten Felder, keine TODO-Platzhalter.
- `Result` + `thiserror`/`anyhow`; **keine Panics im Hot-Path**
  (Aufnahme → STT → Cleanup → Injection). Die Pipeline darf an keiner Stufe
  hart abbrechen.
- Bestehende Dateien editieren statt neue anlegen; minimale, fokussierte
  Changes — kein Refactor über den Auftrag hinaus.
- Nutzersichtbare Änderungen bekommen einen CHANGELOG-Eintrag im selben PR
  (`docs/stability.md`).

## Toolchain

Rust ist via `rust-toolchain.toml` gepinnt (inkl. rustfmt + clippy) — „grün"
ist damit für alle identisch definiert.
