---
name: git-workflow
description: Branches, Commits, Merges und PRs im talker-Team. Nutzen vor jedem Commit, Merge, Push oder PR.
---

# git-workflow

## Branches
- Feature-Branch: `feat/<ticket>-<kurzname>`, `fix/<ticket>-<kurzname>`, `chore/<ticket>-<kurzname>`.
  Legt der Orchestrator an.
- Rollen-Branch: `wt/<rolle>` im eigenen Worktree, bei jedem Start frisch vom Feature-Branch.
- Nie auf `main` committen oder pushen.

## Commits
- Conventional Commits: `type(scope): beschreibung` — `feat`, `fix`, `test`,
  `docs`, `refactor`, `perf`, `chore`, `ci`. Scope = Modul (`audio`, `pipeline`,
  `config`, `ui`, `overlay`, …). Beschreibung deutsch, Präsens, ohne Punkt.
- Ein Commit = eine logische Änderung mit grünem verify:fast (Skill `verify-fast`).
- Nur die eigenen Dateien stagen (`git add <pfade>`), nie `git add -A` ohne Blick auf `git status`.
- Nie `--no-verify`, nie `--amend` auf bereits gemergte Commits, nie Force.

## Merge (nur merger)
`git merge --no-ff wt/<rolle>` in den Feature-Branch; Regeln im merger-Agent.

## Push und PR (nur release-engineer)
```sh
git push -u origin <feature-branch>
gh pr create --draft --base main --head <feature-branch> --title "<type(scope): …>" --body-file <datei>
```
Titel als Conventional Commit (wird beim Squash-Merge die Commit-Message).
Body, knapp und in der Sprache aus `CONTEXT.md`:

```markdown
## Summary

<kleinste Darstellung, die den Kern zeigt: Pseudocode, Aufrufbaum oder Dateibaum>

Ticket: <nummer> · <AKs in einer Zeile>

## Evidence

- **Before:** <Verhalten/Messung/fehlschlagender Test vorher>
  **After:** <nachher; Anzahl neuer Tests; verify:fast grün (n Tests)>

## Merge Danger

**Door:** <one-way oder two-way>

<optional: was schwer umkehrbar ist, z. B. Config-Format>

**Blast Radius:** <ein Wort>
```
Merge nach `main` nur durch den Menschen (Squash).
