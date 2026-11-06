@AGENTS.md

# Claude-Code-spezifisch

- Agent-Definitionen: `.claude/agents/`. Default-Agent dieses Repos ist `orchestrator`
  (nur im Haupt-Checkout, `.claude/settings.local.json`); eine Rolle direkt starten: `claude --agent <rolle>`.
- Gemeinsame Abläufe mehrerer Rollen sind Projekt-Skills in `.claude/skills/`:
  `verify-fast`, `test-first`, `git-workflow`, `ears-requirement`, `field-test`,
  `release-checklist`, `spawn-worker` (nur Orchestrator).
- Rollen kommunizieren über Cross-Session-Messaging (`ListAgents`, `SendMessage`).
  Der Session-Name einer Rolle ist `<rolle>-<ticket>`, z. B. `rust-developer-0026`.
