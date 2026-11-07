#!/bin/sh
# AK-8 Grenzfälle mit den Zahlen der Spec: 7 aktiv → Start ok, 8 aktiv → 9. Start abgelehnt (Default 8 aus team.env),
# TEAM_MAX_SESSIONS=1 ohne laufende Rolle → Start ok. Die 7/8 „laufenden" Rollen liefert ein herdr-Shim in
# `agent list` (acht echte Agenten wären Zeit und Tokens ohne zusätzlichen Erkenntnisgewinn); alles andere
# (Pane, Worktree, Agent-Start) läuft gegen das echte herdr der Session talker-test.
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

inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

# --- =1 ohne laufende Rolle: ein-unter-Grenze → ok (aktive 0 + 1 ≤ 1) ---
inpane "TEAM_MAX_SESSIONS=1 $SP code-reviewer 0001 chore/0001-x"
assert_eq "AK-8 TEAM_MAX_SESSIONS=1 ohne laufende Rolle: Exit 0" 0 "$INPANE_RC"
assert_contains "AK-8 =1: Meldung bereit" "code-reviewer-0001 bereit" "$INPANE_OUT"
inpane "TEAM_MAX_SESSIONS=1 $SP security-auditor 0001 chore/0001-x"
assert_eq "AK-8 TEAM_MAX_SESSIONS=1 mit einer laufenden Rolle: Exit 1" 1 "$INPANE_RC"
inpane "$SP close code-reviewer-0001"
assert_eq "close: Exit 0" 0 "$INPANE_RC"

# --- Shim: `agent list` meldet vorgegebene Namen, alles andere geht ans echte herdr ---
SHIM="$TH_TMP/shim"
mkdir -p "$SHIM"
cat >"$SHIM/herdr" <<EOF
#!/bin/sh
if [ "\$1 \$2" = "agent list" ] && [ -f "$TH_TMP/fake-agents" ]; then
  jq -Rn '[inputs | select(length > 0)] | {result: {agents: map({name: ., pane_id: "fake"})}}' <"$TH_TMP/fake-agents"
  exit 0
fi
exec "$(command -v herdr)" "\$@"
EOF
chmod +x "$SHIM/herdr"
fake() { # <anzahl> — die ersten n Rollen-Sessions von ticket 0001 (ohne test-engineer, das gestartet wird)
  printf '%s\n' product-owner-0001 architect-0001 tech-lead-0001 rust-developer-0001 security-auditor-0001 \
    performance-engineer-0001 merger-0001 release-engineer-0001 | head -n "$1" >"$TH_TMP/fake-agents"
}
withshim() { inpane "PATH=$SHIM:\$PATH $*"; }

# --- 8 aktiv, Default 8 aus team.env des Haupt-Checkouts: 9. Start abgelehnt, nichts geändert ---
assert_eq "Vorbedingung: team.env des Wegwerf-Repos hat den Default 8" "TEAM_MAX_SESSIONS=8" "$(grep '^TEAM_MAX_SESSIONS=' "$TH_REPO/team.env")"
fake 8
before=$(snapshot)
withshim "env -u TEAM_MAX_SESSIONS $SP test-engineer 0001 chore/0001-x"
assert_eq "AK-8 8 aktiv, Default 8: 9. Start abgelehnt (Exit 1)" 1 "$INPANE_RC"
assert_contains "AK-8 Meldung nennt den aktuellen Wert 8" "TEAM_MAX_SESSIONS=8" "$INPANE_OUT"
assert_contains "AK-8 Meldung nennt die Zahl der aktiven Sessions" "8 Rollen-Sessions aktiv" "$INPANE_OUT"
assert_eq "AK-8 abgelehnt: kein Worktree, keine Pane, kein Agent" "$before" "$(snapshot)"
[ ! -d "$TH_WT/test-engineer" ] && ok "AK-8 abgelehnt: Worktree test-engineer fehlt" || fail "AK-8 abgelehnt: Worktree test-engineer fehlt"

# --- 7 aktiv, Default 8: Start ok ---
fake 7
withshim "env -u TEAM_MAX_SESSIONS $SP test-engineer 0001 chore/0001-x"
assert_eq "AK-8 7 aktiv, Default 8: Start ok (Exit 0)" 0 "$INPANE_RC"
assert_contains "AK-8 7 aktiv: Meldung bereit" "test-engineer-0001 bereit" "$INPANE_OUT"
assert_eq "AK-8 7 aktiv: Agent läuft wirklich" 1 "$(th_h agent list | jq '[.result.agents[] | select(.name == "test-engineer-0001")] | length')"

# --- 7 aktiv, TEAM_MAX_SESSIONS=7 → abgelehnt (Grenze exakt) ---
inpane "$SP close test-engineer-0001"
fake 7
before=$(snapshot)
withshim "TEAM_MAX_SESSIONS=7 $SP --instance test-engineer 0001 chore/0001-x"
assert_eq "AK-8 7 aktiv, Max 7: Start abgelehnt" 1 "$INPANE_RC"
assert_eq "AK-8 Max 7 abgelehnt: nichts geändert" "$before" "$(snapshot)"

# --- 8 aktiv, Umgebung 9 → ok ---
fake 8
withshim "TEAM_MAX_SESSIONS=9 $SP test-engineer 0001 chore/0001-x"
assert_eq "AK-8 8 aktiv, Max 9 (Umgebung): Start ok" 0 "$INPANE_RC"

th_summary
