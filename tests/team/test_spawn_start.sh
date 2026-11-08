#!/bin/sh
# T5: spawn.sh — Start über herdr (AK-5), PO-Split und Tab für übrige Rollen (AK-6, Teil), close (AK-10).
. "$(dirname "$0")/lib.sh"

th_begin
th_server

SP='sh .claude/skills/spawn-worker/spawn.sh'
inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

agent_json() { th_h agent get "$1" | jq -r "$2"; }
real() { (cd "$1" && pwd -P); }

# --- Bereit-Zeit überschritten (Vertrauensdialog bleibt offen) → Exit 3, Pane bleibt (Grenzfall) ---
# Der erste Claude der Session zeigt den Dialog (geprüft); darum läuft dieser Fall zuerst.
inpane "SPAWN_READY_TIMEOUT_MS=8000 $SP tech-lead 0001 chore/0001-x"
assert_eq "AK-5 nicht bereit nach Timeout: Exit 3" 3 "$INPANE_RC"
assert_eq "AK-5 Agent bleibt für den Menschen offen (blocked)" blocked "$(agent_json tech-lead-0001 .result.agent.agent_status)"
TL_PANE=$(agent_json tech-lead-0001 .result.agent.pane_id)

# --- AK-10: close ---
inpane "$SP close tech-lead-0001"
assert_eq "AK-10 close: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-10 Agent ist aus agent list verschwunden" 0 \
  "$(th_h agent list | jq '[.result.agents[] | select(.name == "tech-lead-0001")] | length')"
assert_eq "AK-10 Pane ist geschlossen" 0 "$(th_h pane list | jq --arg p "$TL_PANE" '[.result.panes[] | select(.pane_id == $p)] | length')"
inpane "$SP close orchestrator"
assert_eq "AK-10 close orchestrator: Exit 1" 1 "$INPANE_RC"
inpane "$SP close gibtsnicht-0001"
assert_eq "AK-10 close unbekannter Name: Exit 1" 1 "$INPANE_RC"
inpane "$SP close"
assert_eq "AK-10 close ohne Name: Exit 2" 2 "$INPANE_RC"

# --- AK-5/AK-6: product-owner als Pane rechts neben der Aufrufer-Pane ---
th_autotrust
FOCUS_BEFORE=$(th_h pane get "$TH_PANE" | jq -r '.result.pane.focused')
inpane "$SP product-owner 0001 chore/0001-x"
assert_eq "AK-5 product-owner: Exit 0" 0 "$INPANE_RC"
assert_contains "AK-5 letzte Zeile nennt Name und Arbeitsort" "product-owner-0001 bereit in $TH_WT/product-owner" "$(printf '%s\n' "$INPANE_OUT" | tail -n 1)"
assert_eq "AK-5 Agent ist bereit (idle)" idle "$(agent_json product-owner-0001 .result.agent.agent_status)"
PO_PANE=$(agent_json product-owner-0001 .result.agent.pane_id)
assert_eq "AK-6 product-owner liegt rechts neben der Aufrufer-Pane" "$PO_PANE" \
  "$(th_h pane neighbor --direction right --pane "$TH_PANE" | jq -r '.result.neighbor.neighbor_pane_id // empty')"
assert_eq "AK-5 Arbeitsort ist der Rollen-Worktree" "$TH_WT/product-owner" "$(real "$(agent_json product-owner-0001 .result.agent.cwd)")"
assert_eq "AK-5 Rollen-Branch wt/product-owner" wt/product-owner "$(git -C "$TH_WT/product-owner" branch --show-current)"
CMDLINE=$(th_h pane process-info --pane "$PO_PANE" | jq -r '.result.process_info.foreground_processes[].cmdline')
assert_contains "AK-5 Prozess: --agent product-owner" "--agent product-owner" "$CMDLINE"
assert_contains "AK-5 Prozess: --name product-owner-0001" "--name product-owner-0001" "$CMDLINE"
assert_contains "AK-5 Prozess: --permission-mode auto" "--permission-mode auto" "$CMDLINE"
assert_contains "AK-5 Prozess: --add-dir Haupt-Checkout" "--add-dir $TH_REPO" "$CMDLINE"
assert_eq "AK-5 kein Fokuswechsel" "$FOCUS_BEFORE" "$(th_h pane get "$TH_PANE" | jq -r '.result.pane.focused')"

# --- AK-6: übrige Rollen im Tab <ticket> ---
inpane "$SP rust-developer 0001 chore/0001-x"
assert_eq "AK-6 rust-developer: Exit 0" 0 "$INPANE_RC"
RD_TAB=$(agent_json rust-developer-0001 .result.agent.tab_id)
assert_eq "AK-6 Tab heißt <ticket>" 0001 "$(th_h tab get "$RD_TAB" | jq -r '.result.tab.label')"
assert_eq "AK-6 Tab ist nicht der des Orchestrators" 1 "$([ "$RD_TAB" != "$(agent_json product-owner-0001 .result.agent.tab_id)" ] && echo 1 || echo 0)"
assert_eq "AK-5 rust-developer: Arbeitsort Rollen-Worktree" "$TH_WT/rust-developer" "$(real "$(agent_json rust-developer-0001 .result.agent.cwd)")"

# --- darf nicht passieren: keine Ghostty-/Socket-Reste im Skript ---
assert_eq "kein osascript/pgrep/cc-socks/unset im Skript" 0 \
  "$(grep -c -E 'osascript|pgrep|cc-socks|unset' "$TH_SRC/.claude/skills/spawn-worker/spawn.sh" || true)"

th_summary
