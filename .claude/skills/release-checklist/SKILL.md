---
name: release-checklist
description: Ein talker-Release vorbereiten — Abnahme, SemVer-Bump, CHANGELOG-Abschnitt, Tag. Nutzen nur auf ausdrücklichen Auftrag des Menschen.
---

# release-checklist

Grundlage: `docs/stability.md`.

1. `main` ist aktuell und grün (CI und Skill `verify-fast`).
2. **Abnahme** nach Skill `field-test`. Nicht bestanden → Abbruch.
3. **Version bestimmen** aus `[Unreleased]`:
   - **Breaking** vorhanden → MAJOR (vor 1.0.0: MINOR)
   - `Added` vorhanden → MINOR
   - sonst → PATCH
   Vorschlag mit Begründung dem Menschen zeigen, bestätigen lassen.
4. Branch `chore/release-<version>`:
   - `version` in `Cargo.toml`, `cargo build` (aktualisiert `Cargo.lock`).
   - `CHANGELOG.md`: `[Unreleased]` → `[<version>] - <JJJJ-MM-TT>`, neuen leeren
     `[Unreleased]` darüber; Vergleichslinks am Dateiende, falls vorhanden.
   - Deprecations prüfen: Legacy-Pfade, deren Entfernung für diese MAJOR-Version
     angekündigt war, sind ein eigenes Ticket — nicht im Release-PR.
   - Commit `chore(release): <version>`, Push, Draft-PR.
5. Nach dem Squash-Merge durch den Menschen und mit dessen OK:
   `git tag -a v<version> -m "talker <version>"` auf dem Merge-Commit, `git push origin v<version>`.
