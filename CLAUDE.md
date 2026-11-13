@AGENTS.md

# Claude-Code-spezifisch

- Agent-Definitionen: `.claude/agents/`. Einstieg ist `make dev`: Es startet den Orchestrator als herdr-core-Terminal `orchestrator`
  (Default-Agent nur im Haupt-Checkout, `.claude/settings.local.json`) und verbindet das Terminal; eine Rolle direkt starten: `claude --agent <rolle>`.
- Gemeinsame Abläufe mehrerer Rollen sind Projekt-Skills in `.claude/skills/`:
  `verify-fast`, `test-first`, `git-workflow`, `ears-requirement`, `field-test`,
  `release-checklist`, `spawn-worker` (nur Orchestrator).
- Rollen laufen als herdr-core-Terminals (CLI `hc`) und kommunizieren über `sh scripts/team/say.sh <ziel> '<text>'`
  (`hc send` mit Kopfzeile `[talker von <absender>]`); lesend: `hc ls|get|read|wait`.
  Dauerhafte Terminals (Orchestrator, Product-Owner) und flüchtige Worker ordnet der Mensch in der App an.
  Der Terminal-Name einer Rolle ist `<rolle>-<ticket>`, weitere Instanzen `<rolle>-<ticket>-<n>`, z. B. `rust-developer-0026`.
