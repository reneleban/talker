# ADR-0005 — CI auf GitHub-hosted macOS-Runner statt self-hosted Docker

- Status: akzeptiert
- Datum: 2026-07-03
- Kontext: siehe `CONTEXT.md` und `.github/workflows/ci.yml`
- Baut auf: ADR-0001 (Rust, macOS-nativ, direkte native macOS-APIs)

## Kontext und Problemstellung

Für alle Dev-Repos gilt eine repo-übergreifende Konvention (durchgesetzt über den `ci-self-hosted-docker`-Review-Skill): jeder GitHub-Actions-Job soll auf einem self-hosted Runner in einem Docker-Container laufen, nicht auf gehosteten Runnern. talker weicht davon bewusst ab. Der Grund liegt in ADR-0001: talker ist macOS-nativ und ruft direkte native macOS-APIs auf (CoreAudio, AppKit, CGEventTap). Genau das macht Docker-basiertes CI unmöglich — Docker ist Linux-only, und macOS lässt sich aus Apple-Lizenzgründen nicht in Docker oder auf Nicht-Apple-Hardware virtualisieren. Zu entscheiden ist daher, worauf die CI-Jobs laufen und was das Gate abdeckt.

## Entscheidungstreiber

- macOS-nativer Build braucht eine echte macOS-Umgebung mit nativer Toolchain — nicht in Docker/Linux abbildbar (Apple-Lizenz).
- Solo-Public-Repo: minimaler Wartungsaufwand, keine eigene Infrastruktur am Laufen halten.
- Kosten: GitHub-hosted macOS-Runner sind für **public** Repos kostenlos, für **private** Repos ca. 10× teurer als Linux-Runner (macOS-Minuten-Multiplikator).
- Schnelles, verlässliches Gate für jeden PR — ohne dass CI mehrere GB Modell-Artefakte vorhalten muss.

## Betrachtete Optionen

1. **GitHub-hosted macOS-Runner** (`runs-on: macos-latest`), kostenlos für public Repos, wartungsfrei.
2. **Self-hosted Mac** als eigener Runner — erfüllt die self-hosted-Konvention am ehesten, aber eigene Hardware.
3. **Self-hosted Docker** (die Default-Konvention) — für macOS technisch ausgeschlossen.

## Entscheidung

**Option 1: GitHub-hosted macOS-Runner.** Der macOS-Build-Job (`verify`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo build --all-targets --locked`, `cargo test --lib --locked`) läuft auf `runs-on: macos-latest`. Die reinen Text-/Dependency-Analyse-Jobs, die kein macOS brauchen (cargo-deny, cargo-machete, shellcheck), laufen bewusst auf `ubuntu-latest` — Linux-Runner sind günstiger und schneller verfügbar, und cargo-deny ist ohnehin eine Docker-Container-Action, die nur unter Linux läuft. CI wurde erst am 2026-07-03 scharf geschaltet, nachdem das Repo öffentlich gemacht wurde (github.com/reneleban/talker); vorher wäre der macOS-Runner im privaten Repo teuer gewesen, weshalb bis dahin lokales `make verify` plus ein cargo-husky-Pre-Commit-Hook das primäre Gate war.

## Begründung

- Option 3 (self-hosted Docker) ist für macOS technisch unmöglich (siehe oben) — die Konvention kann hier gar nicht greifen.
- Option 2 (self-hosted Mac) hätte die Konvention eher erfüllt, wurde aber wegen Wartungsaufwand verworfen: eigene Hardware am Laufen halten, Updates, Verfügbarkeit. Für ein Solo-Public-Repo steht dem der kostenlose und wartungsfreie GitHub-hosted-Runner gegenüber — der Tribut aus Option 2 lohnt sich nicht.
- Der Kosten-Blocker der macOS-Runner (10× im privaten Repo) entfällt durch das Go-Public; die bewusste Reihenfolge (erst public, dann CI scharf) vermeidet die teure Phase komplett.
- Gate = **verify:fast**: nur `cargo test --lib`, keine Integrationstests. Die Modell-Integrationstests (`tests/cleanup_gemma.rs`, `tests/stt_parakeet.rs`) laufen **nicht** in CI, weil sie mehrere GB lokal liegende Modelle brauchen (Parakeet STT, gemma4:e2b GGUF), die CI nicht vorhält. Diese vollen Tests bleiben manuell/lokal.

## Konsequenzen

- **Positiv:** wartungsfreie, für public Repos kostenlose CI; echte macOS-Umgebung deckt den nativen Build/Toolchain ab; günstige Linux-Runner für Nicht-macOS-Jobs; schnelles PR-Gate ohne GB-schwere Artefakte.
- **Negativ:** bewusste Abweichung von der self-hosted-Docker-Konvention (hier dokumentiert und begründet). Würde das Repo wieder privat, wären macOS-Runner ~10× teurer — dann neu abzuwägen. Modell-Integrationstests sind nicht durch CI abgesichert, sondern nur lokal — Regressionen im STT-/Cleanup-Pfad fallen erst beim manuellen Lauf auf.
- **Verworfen:** self-hosted Mac (Wartungsaufwand für Solo-Repo nicht gerechtfertigt), self-hosted Docker (für macOS technisch ausgeschlossen).
