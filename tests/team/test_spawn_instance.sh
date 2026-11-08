#!/bin/sh
# T9: Instanzen — weitere Sessions derselben Rolle mit --instance (AK-7).
. "$(dirname "$0")/lib.sh"

th_begin
th_server
th_autotrust

SP='sh .claude/skills/spawn-worker/spawn.sh'
FEATURE=chore/0001-x
snapshot() {
  git -C "$TH_REPO" worktree list --porcelain
  git -C "$TH_REPO" branch --list
  th_h pane list | jq -c '[.result.panes[].pane_id]'
  th_h agent list | jq -c '[.result.agents[].name]'
}
agent_json() { th_h agent get "$1" | jq -r "$2"; }
real() { (cd "$1" && pwd -P); }
names() { th_h agent list | jq -r '[.result.agents[].name] | sort | join(" ")'; }

inpane "$SP init 0001 $FEATURE"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

# --- Basisname frei → --instance nimmt ihn (ohne Suffix) ---
inpane "$SP --instance rust-developer 0001 $FEATURE"
assert_eq "AK-7 --instance bei freiem Basisnamen: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-7 Basisname ohne Suffix" "rust-developer-0001" "$(names)"
assert_eq "AK-7 Basis-Worktree" wt/rust-developer "$(git -C "$TH_WT/rust-developer" branch --show-current)"

# --- läuft bereits, ohne --instance → Abbruch vor jeder Änderung ---
before=$(snapshot)
inpane "$SP rust-developer 0001 $FEATURE"
assert_eq "AK-7 ohne --instance: Exit 1" 1 "$INPANE_RC"
assert_contains "AK-7 Meldung 'läuft bereits'" "läuft bereits" "$INPANE_OUT"
assert_eq "AK-7 ohne --instance: nichts geändert" "$before" "$(snapshot)"

# --- 2. Instanz: frisch vom Feature-Branch (der inzwischen weiter ist) ---
git -C "$TH_WT/integration" -c user.email=t@example.invalid -c user.name=t commit -q --allow-empty -m "neuer Stand"
FEATURE_TIP=$(git -C "$TH_REPO" rev-parse "$FEATURE")
inpane "$SP --instance rust-developer 0001 $FEATURE"
assert_eq "AK-7 2. Instanz: Exit 0" 0 "$INPANE_RC"
assert_contains "AK-7 letzte Zeile nennt den Instanz-Namen" "rust-developer-0001-2 bereit in $TH_WT/rust-developer-2" "$(printf '%s\n' "$INPANE_OUT" | tail -n 1)"
assert_eq "AK-7 2. Instanz: Worktree-Branch" wt/rust-developer-2 "$(git -C "$TH_WT/rust-developer-2" branch --show-current)"
assert_eq "AK-7 2. Instanz: Branch frisch vom Feature-Branch" "$FEATURE_TIP" "$(git -C "$TH_WT/rust-developer-2" rev-parse HEAD)"
assert_eq "AK-7 2. Instanz: Arbeitsort" "$TH_WT/rust-developer-2" "$(real "$(agent_json rust-developer-0001-2 .result.agent.cwd)")"
CMDLINE=$(th_h pane process-info --pane "$(agent_json rust-developer-0001-2 .result.agent.pane_id)" | jq -r '.result.process_info.foreground_processes[].cmdline')
assert_contains "AK-7 2. Instanz: Agent bleibt die Rolle" "--agent rust-developer " "$CMDLINE "
assert_contains "AK-7 2. Instanz: Session-Name mit Suffix" "--name rust-developer-0001-2" "$CMDLINE"
assert_eq "AK-7 Basis-Instanz unberührt" wt/rust-developer "$(git -C "$TH_WT/rust-developer" branch --show-current)"

# --- 3. Instanz ---
inpane "$SP --instance rust-developer 0001 $FEATURE"
assert_eq "AK-7 3. Instanz: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-7 drei Instanzen laufen" "rust-developer-0001 rust-developer-0001-2 rust-developer-0001-3" "$(names)"

# --- Lücke: -2 beendet, -3 läuft → nächste freie ist -2 ---
inpane "$SP close rust-developer-0001-2"
assert_eq "AK-7 close -2: Exit 0" 0 "$INPANE_RC"
inpane "$SP --instance rust-developer 0001 $FEATURE"
assert_eq "AK-7 Lücke: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-7 Lücke wird gefüllt (-2 statt -4)" "rust-developer-0001 rust-developer-0001-2 rust-developer-0001-3" "$(names)"

# --- wiederverwendeter Instanz-Worktree mit uncommitteten Änderungen → Abbruch ---
inpane "$SP close rust-developer-0001-3"
echo dreck >"$TH_WT/rust-developer-3/unversioniert.txt"
before=$(snapshot)
inpane "$SP --instance rust-developer 0001 $FEATURE"
assert_eq "AK-7 schmutziger Instanz-Worktree: Exit 1" 1 "$INPANE_RC"
assert_contains "AK-7 Meldung nennt uncommittete Änderungen" "uncommittete" "$INPANE_OUT"
assert_eq "AK-7 schmutziger Instanz-Worktree: nichts geändert" "$before" "$(snapshot)"

th_summary
