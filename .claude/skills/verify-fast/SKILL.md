---
name: verify-fast
description: talker-Gate vor jedem Commit und nach jedem Merge — Build, Tests, Clippy und Formatierung. Nutzen, bevor committet, gemergt oder eine Arbeit als fertig gemeldet wird.
---

# verify-fast

Im aktuellen Worktree ausführen:

```sh
cargo fmt --check
make verify      # cargo build, cargo test, cargo clippy --all-targets -D warnings
```

- **Grün** heißt: alle drei Kommandos Exit 0, keine Warnung von clippy.
- `cargo fmt --check` rot → `cargo fmt`, Diff prüfen, erneut laufen lassen.
- `make verify` führt auch die Modell-Integrationstests aus (`tests/cleanup_gemma.rs`,
  `tests/stt_parakeet.rs`). Fehlen die Modelle lokal, schlagen sie fehl: dann
  `cargo test --lib --bins --examples` als Ersatz und in der Meldung ausdrücklich
  sagen, dass die Modell-Tests nicht liefen.
- Rot → nicht committen, nicht melden „fertig". Ausgabe lesen, Ursache beheben.
- Erster Build in einem neuen Worktree baut alle Abhängigkeiten (llama.cpp) neu
  und dauert lange; das ist kein Fehler.

Der pre-commit-Hook (`.cargo-husky/hooks/pre-commit`) prüft fmt, clippy und
Unit-Tests zusätzlich. Nie mit `--no-verify` umgehen.
