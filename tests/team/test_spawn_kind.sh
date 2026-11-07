#!/bin/sh
# T10: spawn.sh --kind claude|codex (AK-9). Der Codex-Teil braucht ein installiertes codex.
. "$(dirname "$0")/lib.sh"

th_begin
th_server
th_autotrust

SP='sh .claude/skills/spawn-worker/spawn.sh'
snapshot() {
  git -C "$TH_REPO" worktree list --porcelain
  th_h pane list | jq -c '[.result.panes[].pane_id]'
  th_h agent list | jq -c '[.result.agents[].name]'
}
agent_json() { th_h agent get "$1" | jq -r "$2"; }
real() { (cd "$1" && pwd -P); }

inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

# --- Optionsfehler → Exit 2, nichts geändert ---
before=$(snapshot)
inpane "$SP --kind foo tech-lead 0001 chore/0001-x"
assert_eq "AK-9 --kind foo: Exit 2" 2 "$INPANE_RC"
assert_contains "AK-9 Meldung nennt die erlaubten Werte" "claude|codex" "$INPANE_OUT"
inpane "$SP --kind"
assert_eq "AK-9 --kind ohne Wert: Exit 2" 2 "$INPANE_RC"
assert_eq "AK-9 Optionsfehler: nichts geändert" "$before" "$(snapshot)"

# --- Default: claude ---
inpane "$SP tech-lead 0001 chore/0001-x"
assert_eq "AK-9 ohne --kind: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-9 ohne --kind: Kind claude" claude "$(agent_json tech-lead-0001 .result.agent.agent)"
inpane "$SP --kind claude code-reviewer 0001 chore/0001-x"
assert_eq "AK-9 --kind claude: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-9 --kind claude: Kind claude" claude "$(agent_json code-reviewer-0001 .result.agent.agent)"

# --- codex ---
if command -v codex >/dev/null 2>&1; then
  inpane "$SP --kind codex security-auditor 0001 chore/0001-x"
  assert_eq "AK-9 --kind codex: Exit 0" 0 "$INPANE_RC"
  assert_contains "AK-9 codex: letzte Zeile nennt Name und Arbeitsort" "security-auditor-0001 bereit in $TH_WT/integration" "$(printf '%s\n' "$INPANE_OUT" | tail -n 1)"
  assert_eq "AK-9 codex: Kind codex" codex "$(agent_json security-auditor-0001 .result.agent.agent)"
  assert_eq "AK-9 codex: Arbeitsort Integrations-Worktree" "$TH_WT/integration" "$(real "$(agent_json security-auditor-0001 .result.agent.cwd)")"
  PANE=$(agent_json security-auditor-0001 .result.agent.pane_id)
  CMDLINE=$(th_h pane process-info --pane "$PANE" | jq -r '.result.process_info.foreground_processes[].cmdline')
  assert_not_contains "AK-9 codex: keine Claude-Argumente" "--agent" "$CMDLINE"
  assert_not_contains "AK-9 codex: keine Claude-Argumente (--permission-mode)" "--permission-mode" "$CMDLINE"
  assert_contains "AK-9 codex: Prozess ist codex" "codex" "$CMDLINE"
else
  skip "AK-9 codex nicht installiert"
fi

th_summary
