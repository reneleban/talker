---
name: spawn-worker
description: Startet eine talker-Rolle als eigene Claude-Session in einer Ghostty-Kachel neben dem Orchestrator, mit eigenem Worktree, und beendet sie wieder. Nur für den orchestrator.
---

# spawn-worker

Skript: `.claude/skills/spawn-worker/spawn.sh` (Pfade relativ zum Haupt-Checkout).

## Ticket starten (einmal pro Ticket)

```sh
sh .claude/skills/spawn-worker/spawn.sh init <ticket> <feature-branch>
```

Legt den Feature-Branch von `main` an (falls nicht vorhanden) und checkt ihn im
Integrations-Worktree `../talker-wt/integration` aus. Bricht ab, wenn dort
uncommittete Änderungen liegen.

## Rolle starten

1. Auftrag schreiben nach `docs/ticket/auftrag/<ticket>-<rolle>.md`. Inhalt:
   - absoluter Pfad der Ticket-Datei und welcher Abschnitt zu lesen/schreiben ist
   - konkrete Aufgabe dieser Runde (bei Fix-Runden: die Befunde wörtlich)
   - Name des Orchestrators für die Fertigmeldung (aus `ListAgents`, erste Zeile)
2. Starten (wartet, bis die Session erreichbar ist):
   ```sh
   sh .claude/skills/spawn-worker/spawn.sh <rolle> <ticket> <feature-branch>
   ```
3. Auftrag per `SendMessage` an `<rolle>-<ticket>`, erste Zeile:
   `Auftrag <ticket>: lies und bearbeite <absoluter Pfad der Auftrag-Datei>`.
   Ein Start-Prompt funktioniert nicht: In einer Kachel ohne Fokus schickt
   Claude ihn nicht ab.

Die Rolle erscheint als Kachel im Ghostty-Fenster des Orchestrators (erste
rechts daneben, weitere darunter, gleichmäßig verteilt). Arbeitsort je Rolle:
- eigener Worktree `../talker-wt/<rolle>`, Branch `wt/<rolle>` frisch vom Feature-Branch:
  product-owner, architect, rust-developer, test-engineer, release-engineer
- Integrations-Worktree: tech-lead, code-reviewer, security-auditor, performance-engineer, merger

Die Session hat den Haupt-Checkout per `--add-dir` (für die Ticket-Datei) und
startet mit frischem Kontext. Kontext leeren oder Agent wechseln geht nur durch
eine neue Session — nie per Nachricht.

## Rolle beenden

```sh
sh .claude/skills/spawn-worker/spawn.sh close <rolle>-<ticket>
```
Beendet die Session sauber, die Kachel schließt sich. Der Verlauf bleibt über
`claude --resume` erhalten.

## Fehler

- „uncommittete Änderungen": Die Rolle hat nicht committet. Nicht selbst
  aufräumen — der Rolle per `SendMessage` melden oder den Menschen fragen.
- „läuft bereits": erst `close`, dann neu starten.
- „Orchestrator-Terminal nicht gefunden" oder „Kachel konnte nicht geöffnet
  werden": Der Orchestrator läuft nicht in Ghostty, oder macOS hat die
  Automation-Berechtigung (Steuerung von Ghostty) nicht erteilt — Stopp-Punkt,
  dem Menschen melden.
- Neue Worktree-Pfade fragen beim ersten Start nach Ordner-Vertrauen; das
  bestätigt der Mensch in der Kachel.
