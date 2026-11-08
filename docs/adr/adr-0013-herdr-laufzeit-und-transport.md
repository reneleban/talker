# ADR-0013 — herdr als Laufzeit und Transport des Agenten-Teams

- Status: akzeptiert
- Datum: 2026-11-07
- Kontext: Ticket-0043, `AGENTS.md` (Team, Übergabe-Protokoll), `.claude/agents/`,
  `.claude/skills/spawn-worker/`, `scripts/team/`, `team.env`
- Löst ab: ADR-0010 Punkt 2 (Ghostty-Kacheln, Cross-Session-Messaging) und die
  Ghostty-/AppleScript-/Posteingang-Konsequenzen. Alle anderen Punkte von ADR-0010
  gelten weiter.

## Kontext und Problemstellung

ADR-0010 startet jede Rolle als Ghostty-Kachel per AppleScript und lässt sie über
Claude-Codes Cross-Session-Messaging (`SendMessage`, `ListAgents`, Sockets unter
`/tmp/cc-socks`) reden. Das hat drei Folgen: Es läuft nur eine schreibende Rolle
gleichzeitig (ein Worktree je Rolle), nur Claude kann mitarbeiten, und der Mensch
sieht nicht auf einen Blick, welche Rolle arbeitet oder blockiert. Ziel: parallele
Instanzen, ein agentenneutraler Transport, eine wiederverbindbare Arbeitsumgebung
mit Status je Rolle.

Geprüft an `herdr` 0.9.5 (`herdr --version`, `herdr --skill`, CLI-Referenz
v0.9.5 [1]) in Wegwerf-Sessions `arch-probe`/`arch-probe-2`, die danach gestoppt
und gelöscht wurden. Belege stehen im Ticket-0043, Abschnitt `## architect`.

## Entscheidung

1. **Laufzeit**: Das Team läuft in der benannten herdr-Session `talker` (eigener
   Server, eigener Socket unter `~/.config/herdr/sessions/talker/`). Einziger
   Einstieg ist `make dev` (`scripts/team/dev.sh`). Er startet den Server ohne
   `CLAUDE*`-Variablen, legt den Orchestrator als herdr-Agent `orchestrator` an
   und verbindet das Terminal.
2. **Start von Rollen**: `spawn.sh` öffnet eine Pane (`herdr pane split` bzw.
   `herdr tab create`, immer ohne Fokus) und startet den Agenten mit
   `herdr agent start <name> --kind claude|codex --pane <id> -- <argumente>`.
   herdr prüft dabei die Eindeutigkeit des Namens und meldet die Bereitschaft.
   Eigene Prozess- oder Socket-Erkennung entfällt.
3. **Transport**: Jede Nachricht zwischen Rollen geht über
   `scripts/team/say.sh <ziel> '<text>'`. Das Skript ermittelt den Absender aus
   `HERDR_PANE_ID`, setzt die Kopfzeile `[talker von <absender>]` und liefert per
   `herdr agent prompt`. Direktes `herdr agent prompt` und jede Pane-Steuerung
   sind per Deny-Regel gesperrt.
4. **Rückfallweg**: Jede Rolle schreibt ihre Fertigmeldung zusätzlich als
   letzte Zeile in ihren Abschnitt der Akte. Der Orchestrator wartet je
   gestarteter Rolle im Hintergrund mit `herdr agent wait`. Kommt die Rolle zur
   Ruhe, ohne dass eine Nachricht da ist, gilt die Zeile in der Akte.
5. **Rollen ohne Bash** (tech-lead): Sie bekommen `Bash`. Ein `PreToolUse`-Hook
   im Frontmatter lässt nur `sh scripts/team/say.sh <ziel> '<text>'` durch und
   blockiert alles andere mit Exit 2.
6. **Parallelität**: Instanzen `<rolle>-<ticket>-<n>` (n ≥ 2) mit eigenem
   Worktree `../talker-wt/<rolle>-<n>` und Branch `wt/<rolle>-<n>`. Obergrenze
   `TEAM_MAX_SESSIONS` aus `team.env`, Umgebungsvariable gleichen Namens geht vor.
   Fix-Runden: max. 3.

## Betrachtete Optionen

### Start-Mechanik

1. **`pane split` + `herdr agent start --kind …`** — gewählt. herdr lehnt einen
   vergebenen Namen ab, bevor es etwas startet (`agent_name_taken`, Pane bleibt
   an der Shell). Der Aufruf kehrt erst zurück, wenn der Agent bereit ist. Bei
   einem Dialog meldet er sofort `agent_not_ready`. Die Argumente nach `--`
   reicht herdr an `claude` bzw. `codex` durch. Codex läuft über denselben Weg.
2. **`pane run` mit eigenem Kommando** (`unset -m 'CLAUDE*'; exec claude …`)
   und danach `agent rename`. Verworfen: Name, Eindeutigkeit und
   Bereit-Erkennung müssten wir selbst bauen, und es gäbe ein Zeitfenster, in dem
   der Agent ohne Namen läuft.

### Rollen ohne Bash

1. **`Bash` + enge Allow-Regel in `settings.json`** — allein unzureichend: Im
   Permission-Modus `auto` gehen Bash-Befehle ohne passende Regel an den
   Klassifizierer und werden nicht abgelehnt [2]. Die Rolle hätte praktisch
   freies Bash.
2. **`tools: Bash(sh scripts/team/say.sh *)` im Frontmatter** — nicht möglich:
   `tools` kennt keine Bash-Muster, und ein Muster in `disallowedTools` entfernt
   das ganze Tool [3].
3. **`Bash` + `PreToolUse`-Hook im Frontmatter** — gewählt. Exit 2 blockiert den
   Aufruf [4]. Frontmatter-Hooks greifen auch bei `claude --agent` [3]. Damit ist
   die Grenze technisch, nicht nur Verhaltensregel.
4. **Keine eigene Meldung, nur Akte + `herdr agent wait`**: kommt ohne neue
   Rechte aus. Verworfen, weil AK-15 eine Meldung jeder Rolle verlangt. Der Weg
   bleibt als Rückfall (Entscheidung 4).

### ADR-0010 ändern oder neues ADR

Neues ADR gewählt. ADR-0010 bleibt als Begründung für Rollen, Akte und Gates
lesbar. Nur Punkt 2 und die Ghostty-Konsequenzen sind abgelöst, sichtbar im
Status von ADR-0010.

## Konsequenzen

- Agent-Namen, Pane- und Tab-IDs gelten je herdr-Server, also je Session.
  `talker` und `talker-test` stören sich nicht (geprüft: gleicher Name `probe`
  lief gleichzeitig in zwei Sessions). Tests laufen in `talker-test`
  (`TEAM_SESSION=talker-test`).
- Panes erben die Umgebung des **Servers**, nicht die des Aufrufers. herdr
  entfernt nur einen Teil der Claude-Marker. In einer Pane eines Servers, der aus
  einer Claude-Session gestartet wurde, standen noch `CLAUDE_CODE_AGENT`,
  `CLAUDE_EFFORT`, `CLAUDE_PID` und `CLAUDE_CODE_MESSAGING_SOCKET`. Deshalb
  startet nur `dev.sh` Server, und zwar ohne `CLAUDE*`.
- `herdr agent prompt` mitten in einem laufenden Turn wird zugestellt und
  landet im laufenden Turn. Im Versuch beantwortete der Agent nur die neue
  Nachricht; die Antwort auf den laufenden Auftrag fehlte (einmal beobachtet,
  Haiku). Deshalb gibt es den Rückfallweg über die Akte.
- Ein Erfolg von `agent prompt` ohne `--wait` heißt nur: geschrieben. Er
  beweist keinen gestarteten Turn [1]. `say.sh` wartet bewusst nicht, sonst
  hinge der Absender am Turn des Empfängers.
- Der neue Transport ist an herdr gebunden. Rückweg ist ein Revert.
- Die Kopfzeile ist eine Verhaltensregel, keine technische Sperre. Die Deny-Regeln auf direktes
  `herdr …` und der PreToolUse-Hook `herdr-guard.sh` (lehnt jeden Bash-Befehl mit dem Token `herdr`
  ab außer `herdr agent list|get|read|wait`) machen das Weglassen schwerer; sie sind aber
  keine Sicherheitsgrenze (verschleierte Aufrufe, andere Wege auf den herdr-Socket). Deshalb gilt die
  Spec-Freigabe nur, wenn der Mensch sie auf eine `AskUserQuestion` des PO gibt, nie aus freiem
  Pane-Text. Freigaben gelten nur über die Akte. Folge des Hooks: Auch ein Befehl, der `herdr` nur
  als Text enthält (z. B. eine Commit-Nachricht), wird abgelehnt.
- Nach einem Neustart des herdr-Servers stellt herdr das Layout wieder her und
  kann Claude per `claude --resume <id>` wieder aufnehmen, dann aber ohne
  herdr-Namen [5]. `dev.sh` benennt einen solchen Orchestrator um, statt einen
  zweiten zu starten (nicht geprüft, Annahme). Rollen startet der Orchestrator
  neu.
- Die Ghostty-Automation-Berechtigung entfällt. Der Ordner-Vertrauensdialog
  neuer Worktrees bleibt; herdr meldet ihn als `blocked`.
- Parallele Instanzen bauen in eigenen `target/`-Verzeichnissen; Ressourcen
  paralleler Builds sind nicht gelöst (Ticket-0043, Out of Scope).

## Quellen

1. herdr CLI-Referenz v0.9.5,
   https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/cli-reference.mdx
2. Claude Code, Permission modes (Auto mode),
   https://code.claude.com/docs/en/permission-modes
3. Claude Code, Subagents (`tools`, `disallowedTools`, `--agent`),
   https://code.claude.com/docs/en/sub-agents
4. Claude Code, Hooks (PreToolUse, Exit-Code 2),
   https://code.claude.com/docs/en/hooks
5. herdr Session state and restore v0.9.5,
   https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/session-state.mdx
