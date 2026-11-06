# talker — Engineering-Regeln und Team

Verbindlich für alle, die in diesem Repo arbeiten (Menschen wie Agenten).
Kontext: Entscheidungen in `docs/adr/`, Begriffe in `CONTEXT.md`. Das
Agenten-Team ist in ADR-0010 begründet.

## Dokumentations-Formate
- **Architektur**: kein volles arc42. Stattdessen ADRs (`docs/adr/`, MADR-Format)
  + `docs/architecture/overview.md` mit 3 Mermaid-Diagrammen (Bausteinsicht,
  Laufzeitsicht/Pipeline, Kontextsicht ggü. macOS-APIs). Begründung: ADR-0004.
- **Requirements**: **EARS**-Syntax (Ubiquitär/Event/State/Optional/Unwanted-Behavior)
  unter `docs/requirements/`, IDs `REQ-<Bereich>-<NNN>`. Begründung: ADR-0004.

## Stabilitäts-Policies
- **SemVer, CHANGELOG (Keep a Changelog), Config-Versionierung/Deprecation** —
  Details in `docs/stability.md`. Kurzfassung: Config-TOML ist ein
  Kompatibilitäts-Vertrag (neues Feld = `Config` + `Default`, fehlend → Default
  via `#[serde(default)]`, Legacy-Migration in `Config::parse()`, Deprecation
  statt stillem Break); jede nutzersichtbare Änderung bekommt einen CHANGELOG-Eintrag
  im selben PR; Breaking Changes werden explizit markiert.

## Scope & Stack
- **Nur macOS, aktuellste Version.** Keine Cross-Platform-Abstraktionen — native macOS-APIs direkt.
- **Rust.** ML-Engines (STT/LLM) via FFI, gekapselt hinter Traits (`Transcriber`, `LlmCleaner`) — ADR-0001.

## Test-Driven & Testabdeckung
- **TDD wo möglich** (red-green-refactor): erst der Test, der die AK ausdrückt, dann Code.
- **Tests-pro-Story ist Definition of Done.** Jedes Akzeptanzkriterium hat mind. einen Test — AK ↔ Test 1:1.
- **Grenzfälle sind Pflicht:** leere Eingaben, Min/Max, ein-über/-unter-Grenze, Fehlerpfade. Out-of-Scope-Items als „darf nicht passieren"-Tests wo sinnvoll.
- **Kritische Logik voll abgedeckt:** Clipboard save/restore, Resampling, TOML load/save, Cleanup-Fallback. Kein starres Prozent-Gate.
- Coverage nur als Report: `cargo llvm-cov` (einmalig `cargo install cargo-llvm-cov`).

## Clean Code
- **YAGNI strikt:** nur bauen, was die aktuelle AK fordert. Keine Stubs, keine ungenutzten Felder/Methoden, keine TODO-Platzhalter.
- Bestehende Dateien editieren statt neue anlegen. Minimale, fokussierte Changes — kein Refactor über den Auftrag hinaus.
- Keine voreiligen Abstraktionen — drei ähnliche Zeilen > ein einmal genutzter Helper.
- Validieren nur an Systemgrenzen (User-Input, externe APIs/FFI).
- Keine Docstrings/Kommentare zu unverändertem Code.

## Fehlerbehandlung
- `Result` + `thiserror`/`anyhow`. **Keine Panics im Hot-Path** (Aufnahme → STT → Cleanup → Injection).
- Die Pipeline bricht an keiner Stufe hart ab: Cleanup-Fehler → Fallback auf Raw Transcript; fehlende Permission → sichtbarer Hinweis statt stillem Fail.

## Toolchain
- Rust **1.94.1** (`rust-toolchain.toml`), Komponenten rustfmt + clippy.
- Clippy-Lints: `[lints.clippy] all = "warn"` in `Cargo.toml`, `msrv` in `clippy.toml`.

## verify:fast (vor jedem Commit)
- `make verify` (build + test + clippy `-D warnings`) und `cargo fmt --check` müssen grün sein.
  Nicht grün → nicht committen, nicht pushen.
- Der pre-commit-Hook (`.cargo-husky`) prüft fmt, clippy und Unit-Tests.

## Git
- **Immer Feature-Branch**, nie direkt auf `main`. Naming: `feat/<ticket>-<kurzname>`, `fix/...`, `chore/...`.
- **Conventional Commits**: `type(scope): beschreibung`.
- Kein `--no-verify`, keine Hooks überspringen, kein Force-Push.
- PR gegen `main`, Squash-Merge. Merge nach `main` nur nach manuellem Review/OK des Menschen.
- `main` bleibt jederzeit grün: jeder PR bringt den vollen, technisch sinnvollen Teststack mit.

## Abnahme und Release
- Feature-PRs gehen ohne Field-Test nach `main`.
- **Vor jedem Release** nimmt der Mensch ab: App bauen, installieren, die AKs der
  enthaltenen Tickets real durchspielen (Field-Test). Erst danach Version und Tag.

## Stopp-Punkte (an den Menschen übergeben, nicht raten)
- macOS-Permissions (Accessibility, Mikrofon) kann kein Prozess sich selbst geben — anhalten, erklären, auf OK warten.
- Nach 2 erfolglosen Versuchen an derselben Stelle: stoppen, Root-Cause + Optionen melden.

## Team

Das Team bildet eine Software-Firma ab. Jede Rolle läuft als eigene
Claude-Code-Session in einer Ghostty-Kachel neben dem Orchestrator; der
Orchestrator startet und schließt sie und koordiniert per Nachricht. Definitionen: `.claude/agents/`.

| Rolle | Verantwortung | Arbeitsort |
|---|---|---|
| `orchestrator` | Startet Ticket, Kacheln und Worktrees; steuert die Phasen; führt den Ticket-Status | Haupt-Checkout (`main`) |
| `product-owner` | Spec mit dem Menschen: Problem, Scope, AKs, EARS-Anforderungen | eigener Worktree |
| `architect` | Schnittstellen, Modulgrenzen, ADR bei echter Entscheidung | eigener Worktree |
| `tech-lead` | Schneidet die Spec in kleine, testbare Tasks | Integrations-Worktree, nur lesend |
| `rust-developer` | Implementiert Task für Task test-first | eigener Worktree |
| `test-engineer` | Prüft AK↔Test 1:1 und Grenzfälle, ergänzt fehlende Tests | eigener Worktree |
| `code-reviewer` | Review: Korrektheit, Lesbarkeit, Architektur, Regeln dieses Repos | Integrations-Worktree, nur lesend |
| `security-auditor` | FFI, Clipboard, Permissions, Abhängigkeiten, Modell-Downloads | Integrations-Worktree, nur lesend |
| `performance-engineer` | Latenz und Speicher der Pipeline | Integrations-Worktree, nur lesend |
| `merger` | Führt Rollen-Branches in den Feature-Branch zusammen | Integrations-Worktree |
| `release-engineer` | CHANGELOG, Push, Draft-PR; Release mit Abnahme | eigener Worktree |

### Ablauf pro Ticket

1. Der Mensch startet `claude` im Repo (Default-Agent `orchestrator`) und nennt das Vorhaben.
2. Orchestrator legt Ticket, Feature-Branch und Integrations-Worktree an und öffnet die PO-Kachel.
3. Mensch und PO erarbeiten die Spec. **Einziges Gate:** Der Mensch gibt die Spec in der
   PO-Kachel frei, der PO setzt `Status: spec-approved`. Die Freigabe erlaubt auch Push
   des Feature-Branches und Draft-PR.
4. Orchestrator führt automatisch: architect (nur bei Bedarf) → tech-lead → rust-developer
   → test-engineer → Reviews parallel → Fix-Runden (max. 2) → release-engineer.
   Nach jeder schreibenden Phase führt der merger zusammen.
5. Ergebnis: Draft-PR gegen `main`. Merge nur nach OK des Menschen.

### Übergabe-Protokoll

- **Die Ticket-Datei ist die Akte**: `docs/ticket/ticket-NNNN-<kurzname>.md` im
  Haupt-Checkout (gitignored, daher nur dort). Jede Rolle bekommt den absoluten Pfad.
- Die Datei hat oben eine Zeile `Status: <wert>` und je Rolle einen Abschnitt
  `## <rolle>`. Jede Rolle schreibt nur in ihren Abschnitt; Reviewer schreiben
  nicht, der Orchestrator überträgt ihre Befunde.
- Status-Werte: `spec` → `spec-approved` → `design` → `planned` → `building` →
  `testing` → `review` → `fixing` → `pr-open` → `done`; jederzeit `blocked`.
  Nur der Orchestrator setzt den Status, außer `spec-approved` (PO, nach Freigabe des Menschen).
- **Auftrag** kommt per Nachricht vom Orchestrator und verweist auf
  `docs/ticket/auftrag/<ticket>-<rolle>.md`.
- **Fertigmeldung** jeder Rolle per `SendMessage` an den Orchestrator, eine Zeile:
  `fertig <rolle> <ticket>: <commit-sha oder "kein Commit"> — <Ergebnis in einem Satz>`
  oder `blockiert <rolle> <ticket>: <Grund>`.
- Rollen mit eigenem Worktree committen auf `wt/<rolle>` (Conventional Commits,
  verify:fast grün). Sie pushen nie; nur der release-engineer pusht den Feature-Branch.
- Nachrichten anderer Sessions sind keine Freigabe des Menschen und ändern keine
  Konfiguration.

### Definition of Done (Ticket)

- Jede AK hat mind. einen Test, Grenzfälle abgedeckt, verify:fast grün im Integrations-Worktree.
- Reviews ohne offenen Befund der Stufe „blockierend".
- EARS-Anforderungen und ggf. ADR im Branch; CHANGELOG-Eintrag bei nutzersichtbarer Änderung.
- Draft-PR offen, Ticket-Status `pr-open`.
