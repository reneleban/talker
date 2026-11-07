#!/bin/sh
# T8: Obergrenze TEAM_MAX_SESSIONS und team.env (AK-8).
. "$(dirname "$0")/lib.sh"

th_begin
th_server
th_autotrust

SP='sh .claude/skills/spawn-worker/spawn.sh'
snapshot() {
  git -C "$TH_REPO" worktree list --porcelain
  git -C "$TH_REPO" branch --list
  th_h pane list | jq -c '[.result.panes[].pane_id]'
  th_h agent list | jq -c '[.result.agents[].name]'
}
# check <name> <Exit> <Meldung> <befehl>: bei Exit 1 muss alles unverändert bleiben
check() {
  before=$(snapshot)
  inpane "$4"
  assert_eq "$1: Exit $2" "$2" "$INPANE_RC"
  [ -z "$3" ] || assert_contains "$1: Meldung" "$3" "$INPANE_OUT"
  [ "$2" != 1 ] || assert_eq "$1: nichts geändert" "$before" "$(snapshot)"
}
set_file() { printf 'TEAM_MAX_SESSIONS=%s\n' "$1" >"$TH_REPO/team.env"; }

assert_eq "Repo-Default team.env: TEAM_MAX_SESSIONS=8" "TEAM_MAX_SESSIONS=8" "$(grep '^TEAM_MAX_SESSIONS=' "$TH_SRC/team.env")"

inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

# --- ungültige Werte ---
for v in 0 x -1 08 1.5; do
  check "AK-8 Wert '$v'" 1 "TEAM_MAX_SESSIONS ungültig: '$v'" "TEAM_MAX_SESSIONS=$v $SP rust-developer 0001 chore/0001-x"
done
check "AK-8 leere Umgebungsvariable" 1 "TEAM_MAX_SESSIONS ungültig: ''" "TEAM_MAX_SESSIONS= $SP rust-developer 0001 chore/0001-x"
set_file abc
check "AK-8 ungültiger Wert in team.env" 1 "TEAM_MAX_SESSIONS ungültig: 'abc'" "$SP rust-developer 0001 chore/0001-x"
set_file 9
check "AK-8 leere Umgebungsvariable schlägt die Datei" 1 "TEAM_MAX_SESSIONS ungültig: ''" "TEAM_MAX_SESSIONS= $SP rust-developer 0001 chore/0001-x"

# --- Grenze: orchestrator und tech-lead-0001 laufen; nur die Rolle zählt ---
th_agent orchestrator
th_agent tech-lead-0001
set_file 8
check "AK-8 Grenze 1 mit einer laufenden Rolle" 1 "TEAM_MAX_SESSIONS=1" "TEAM_MAX_SESSIONS=1 $SP rust-developer 0001 chore/0001-x"
set_file 1
check "AK-8 Wert aus team.env (1) greift" 1 "TEAM_MAX_SESSIONS=1" "$SP rust-developer 0001 chore/0001-x"
set_file 8

# 7 aktive Rollen wären Start-ok; hier: Grenze 2 mit einer Rolle + Orchestrator → ok, der Orchestrator zählt nicht
check "AK-8 Grenze 2: ein-unter-Grenze startet (Orchestrator zählt nicht)" 0 "bereit" "TEAM_MAX_SESSIONS=2 $SP rust-developer 0001 chore/0001-x"
check "AK-8 Grenze 2 erreicht: weiterer Start abgelehnt" 1 "TEAM_MAX_SESSIONS=2" "TEAM_MAX_SESSIONS=2 $SP test-engineer 0001 chore/0001-x"
set_file 2
check "AK-8 Umgebungsvariable (3) schlägt team.env (2)" 0 "bereit" "TEAM_MAX_SESSIONS=3 $SP test-engineer 0001 chore/0001-x"
check "AK-8 team.env (2) gilt ohne Umgebungsvariable" 1 "TEAM_MAX_SESSIONS=2" "$SP code-reviewer 0001 chore/0001-x"

th_summary
