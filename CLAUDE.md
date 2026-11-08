@AGENTS.md

# Claude-Code-spezifisch

- Agent-Definitionen: `.claude/agents/`. Einstieg ist `make dev`: Es startet die herdr-Session `talker`
  mit dem Default-Agenten `orchestrator` (nur im Haupt-Checkout, `.claude/settings.local.json`) im ersten Tab
  und verbindet das Terminal; eine Rolle direkt starten: `claude --agent <rolle>`.
- Gemeinsame Abläufe mehrerer Rollen sind Projekt-Skills in `.claude/skills/`:
  `verify-fast`, `test-first`, `git-workflow`, `ears-requirement`, `field-test`,
  `release-checklist`, `spawn-worker` (nur Orchestrator).
- Rollen laufen als herdr-Panes der Session `talker` und kommunizieren über `sh scripts/team/say.sh <ziel> '<text>'`
  (herdr `agent prompt` mit Kopfzeile `[talker von <absender>]`); lesend: `herdr agent list|get|read|wait`.
  Der Session-Name einer Rolle ist `<rolle>-<ticket>`, weitere Instanzen `<rolle>-<ticket>-<n>`, z. B. `rust-developer-0026`.
