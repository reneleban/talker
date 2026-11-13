---
name: spawn-worker
description: Startet eine talker-Rolle als eigene Session in einem herdr-core-Terminal, mit eigenem Worktree, und beendet sie wieder. Nur für den orchestrator.
---

# spawn-worker

Skript: `.claude/skills/spawn-worker/spawn.sh` (Pfade relativ zum Haupt-Checkout). Läuft nur in einem
herdr-core-Terminal (`HC_TERMINAL_ID` gesetzt); Einstieg ist `make dev` (startet den Orchestrator und
verbindet das Terminal). Außerhalb von herdr-core bricht das Skript mit einem Hinweis auf `make dev` ab.

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
   - Fertigmeldung per `sh scripts/team/say.sh orchestrator '…'` (der Orchestrator heißt immer `orchestrator`)
2. Abschnitt `## <rolle>` bzw. `## <rolle>-<n>` in der Ticket-Datei anlegen, dann starten
   (wartet, bis der Agent als bereit und nicht blockiert gemeldet wird, max. 60 s):
   ```sh
   sh .claude/skills/spawn-worker/spawn.sh [--kind claude|codex] [--instance] <rolle> <ticket> <feature-branch>
   ```
   Die letzte Zeile der Ausgabe nennt den Namen der Session: `<name> bereit in <dir>`.
3. Auftrag senden, erste Zeile:
   `sh scripts/team/say.sh <name> 'Auftrag <ticket>: lies und bearbeite <absoluter Pfad der Auftrag-Datei>'`
   (Text ohne `'`; das Skript stellt `[talker von <absender>]` voran). Ein Start-Prompt ist nicht nötig.

Optionen (nur vor `<rolle>`):
- `--instance`: eine weitere Session derselben Rolle, nur für Rollen mit eigenem Worktree. Läuft die
  Basis (`<rolle>-<ticket>`) nicht, wird sie genommen, sonst die kleinste freie `<rolle>-<ticket>-<n>`
  (n ≥ 2) mit Worktree `../talker-wt/<rolle>-<n>` und Branch `wt/<rolle>-<n>` frisch vom Feature-Branch.
  Ohne `--instance` bricht ein zweiter Start mit „läuft bereits" ab.
- `--kind claude|codex` (Default `claude`): Codex startet ohne Rollen-Argumente (Codex-Setup ist ein Folgeticket).

Die Obergrenze gleichzeitiger Rollen-Sessions steht in `team.env` (`TEAM_MAX_SESSIONS`, Default 8); eine
Umgebungsvariable gleichen Namens überschreibt sie. Der Orchestrator zählt nicht. Wäre die Grenze danach
überschritten, bricht das Skript vor jeder Änderung ab.

## Wo die Rolle läuft

Als eigenes herdr-core-Terminal mit den Labels `ticket=<ticket>` und `role=<rolle>`. Wo es in der App
erscheint (dauerhafte Terminals wie product-owner, flüchtige Worker), ordnet der Mensch an; das Skript
platziert nichts. Arbeitsort je Rolle:
- eigener Worktree `../talker-wt/<rolle>`, Branch `wt/<rolle>` frisch vom Feature-Branch:
  product-owner, architect, rust-developer, test-engineer, release-engineer
- Integrations-Worktree: tech-lead, code-reviewer, security-auditor, performance-engineer, merger

Die Session hat den Haupt-Checkout per `--add-dir` (für die Ticket-Datei) und
startet mit frischem Kontext. Kontext leeren oder Agent wechseln geht nur durch
eine neue Session — nie per Nachricht.

## Exit-Codes

- **0** bereit — die Session läuft und ist laut `hc` bereit.
- **1** Abbruch vor jeder Änderung: Validierung, Obergrenze, „läuft bereits", unsauberer Worktree,
  außerhalb von herdr-core, unbekannter Name bei `close`.
- **2** Aufruf- oder Optionsfehler.
- **3** Terminal gestartet, aber nach 60 s nicht bereit (es bleibt offen für den Menschen,
  meist ein Dialog), oder `hc`-Fehler nach einer Änderung (die Meldung nennt, was schon angelegt ist).

## Rolle beenden

```sh
sh .claude/skills/spawn-worker/spawn.sh close <rolle>-<ticket>
```
Beendet das Terminal der Session. `orchestrator` lässt sich so nicht beenden. Der Verlauf
bleibt über `claude --resume` erhalten.

## Fehler

- „uncommittete Änderungen": Die Rolle hat nicht committet. Nicht selbst aufräumen — der Rolle per
  `say.sh` melden oder den Menschen fragen.
- „läuft bereits": erst `close`, dann neu starten, oder mit `--instance` eine weitere Instanz.
- „Obergrenze erreicht": eine Session beenden oder `TEAM_MAX_SESSIONS` anheben (der Mensch).
- Exit 3 / `blocked`: Neue Worktree-Pfade fragen beim ersten Start nach Ordner-Vertrauen; das
  bestätigt der Mensch im Terminal. Dem Menschen melden, nicht selbst beantworten.
