#!/bin/sh
# T6: Tab-Layout — max. 4 Panes je Tab <ticket> (rechts, unten von 1, unten von 2), danach <ticket>-2 (AK-6).
. "$(dirname "$0")/lib.sh"

th_begin
th_server
th_autotrust

SP='sh .claude/skills/spawn-worker/spawn.sh'
inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

agent_json() { th_h agent get "$1" | jq -r "$2"; }
tab_of() { agent_json "$1" .result.agent.tab_id; }
label_of() { th_h tab get "$1" | jq -r '.result.tab.label'; }
rect() { # <agent> <feld>
  p=$(agent_json "$1" .result.agent.pane_id)
  th_h pane layout --pane "$p" | jq -r --arg p "$p" ".result.layout.panes[] | select(.pane_id == \$p) | .rect.$2"
}
spawn() { inpane "$SP $1 0001 chore/0001-x"; assert_eq "AK-6 $1: Exit 0" 0 "$INPANE_RC"; }

spawn rust-developer
spawn test-engineer
spawn code-reviewer
spawn security-auditor

T1=$(tab_of rust-developer-0001)
assert_eq "AK-6 Tab der ersten Rolle heißt <ticket>" 0001 "$(label_of "$T1")"
for r in test-engineer code-reviewer security-auditor; do
  assert_eq "AK-6 $r liegt im Tab <ticket>" "$T1" "$(tab_of "$r-0001")"
done
assert_eq "AK-6 Tab <ticket> hat 4 Panes" 4 "$(th_h tab get "$T1" | jq -r '.result.tab.pane_count')"
assert_eq "AK-6 Pane 2 liegt rechts von Pane 1" 1 "$([ "$(rect test-engineer-0001 x)" -gt "$(rect rust-developer-0001 x)" ] && echo 1 || echo 0)"
assert_eq "AK-6 Pane 3 liegt unter Pane 1" 1 \
  "$([ "$(rect code-reviewer-0001 y)" -gt "$(rect rust-developer-0001 y)" ] && [ "$(rect code-reviewer-0001 x)" = "$(rect rust-developer-0001 x)" ] && echo 1 || echo 0)"
assert_eq "AK-6 Pane 4 liegt unter Pane 2" 1 \
  "$([ "$(rect security-auditor-0001 y)" -gt "$(rect test-engineer-0001 y)" ] && [ "$(rect security-auditor-0001 x)" = "$(rect test-engineer-0001 x)" ] && echo 1 || echo 0)"
# Integrations-Rolle arbeitet im Integrations-Worktree (AK-5)
CWD=$(agent_json code-reviewer-0001 .result.agent.cwd)
assert_eq "AK-5 code-reviewer: Arbeitsort Integrations-Worktree" "$TH_WT/integration" "$(cd "$CWD" && pwd -P)"

# --- 5. Rolle → neuer Tab <ticket>-2 ---
spawn performance-engineer
T2=$(tab_of performance-engineer-0001)
assert_eq "AK-6 5. Rolle: anderer Tab" 1 "$([ "$T2" != "$T1" ] && echo 1 || echo 0)"
assert_eq "AK-6 5. Rolle: Tab heißt <ticket>-2" 0001-2 "$(label_of "$T2")"
assert_eq "AK-6 Tab <ticket> bleibt bei 4 Panes" 4 "$(th_h tab get "$T1" | jq -r '.result.tab.pane_count')"

# --- A4: letzte Pane eines Tabs schließen ---
inpane "$SP close performance-engineer-0001"
assert_eq "A4 close: Exit 0" 0 "$INPANE_RC"
assert_eq "A4 Tab <ticket>-2 ist mit seiner letzten Pane verschwunden" 0 \
  "$(th_h tab list | jq '[.result.tabs[] | select(.label == "0001-2")] | length')"

# --- Lücke im ersten Tab wird zuerst gefüllt ---
inpane "$SP close code-reviewer-0001"
assert_eq "Lücke: Tab <ticket> hat 3 Panes" 3 "$(th_h tab get "$T1" | jq -r '.result.tab.pane_count')"
spawn merger
assert_eq "Lücke: neue Rolle füllt den Tab <ticket>" "$T1" "$(tab_of merger-0001)"
assert_eq "Lücke: Tab <ticket> hat wieder 4 Panes" 4 "$(th_h tab get "$T1" | jq -r '.result.tab.pane_count')"

# --- voller Tab, <ticket>-2 ist frei → wieder <ticket>-2 ---
spawn release-engineer
assert_eq "AK-6 neue Rolle bei vollem Tab: <ticket>-2" 0001-2 "$(label_of "$(tab_of release-engineer-0001)")"

th_summary
