#!/bin/sh
# spawn.sh gegen Fake-hc und Wegwerf-Repo: Prüfungen vor jeder Änderung, Start, Instanzen, Obergrenze, close.
. "$(dirname "$0")/lib.sh"

th_begin
cd "$TH_REPO" || exit 1
SPAWN='sh .claude/skills/spawn-worker/spawn.sh'
export TEAM_MAX_SESSIONS=3 SPAWN_READY_TIMEOUT_S=1
FEATURE=feat/0099-test

run() { : >"$LOG"; OUT=$("$@" 2>&1); RC=$?; }

run $SPAWN init 0099 "$FEATURE"
assert_eq "init: Exit 0" 0 "$RC"
[ -d "$TH_WT/integration" ] && ok "init: Integrations-Worktree angelegt" || fail "init: Integrations-Worktree angelegt"
run $SPAWN init 99 "$FEATURE"
assert_eq "init: Ticket nicht vierstellig: Exit 1" 1 "$RC"
run $SPAWN init 0099 main
assert_eq "init: Feature-Branch ungültig: Exit 1" 1 "$RC"

# --- Prüfungen vor jeder Änderung: kein hc run, kein Worktree ---
run $SPAWN;                                      assert_eq "ohne Argumente: Exit 2" 2 "$RC"
run $SPAWN --kind;                               assert_eq "--kind ohne Wert: Exit 2" 2 "$RC"
run $SPAWN --kind foo rust-developer 0099 "$FEATURE"; assert_eq "unbekannter --kind: Exit 2" 2 "$RC"
run $SPAWN --foo rust-developer 0099 "$FEATURE"; assert_eq "unbekannte Option: Exit 2" 2 "$RC"
run env -u HC_TERMINAL_ID $SPAWN rust-developer 0099 "$FEATURE"
assert_eq "außerhalb von herdr-core: Exit 1" 1 "$RC"
assert_contains "Hinweis auf make dev" "make dev" "$OUT"
run $SPAWN koch 0099 "$FEATURE";                 assert_eq "unbekannte Rolle: Exit 1" 1 "$RC"
run $SPAWN rust-developer 99 "$FEATURE";         assert_eq "Ticket nicht vierstellig: Exit 1" 1 "$RC"
run $SPAWN rust-developer 0099 main;             assert_eq "Feature-Branch ungültig: Exit 1" 1 "$RC"
run $SPAWN --instance tech-lead 0099 "$FEATURE"; assert_eq "--instance für tech-lead: Exit 1" 1 "$RC"
run env TEAM_MAX_SESSIONS=0 $SPAWN rust-developer 0099 "$FEATURE"
assert_eq "TEAM_MAX_SESSIONS=0 ungültig: Exit 1" 1 "$RC"
assert_eq "Prüfungen rufen hc run nicht auf" 0 "$(calls '^run')"
[ ! -d "$TH_WT/rust-developer" ] && ok "Prüfungen legen keinen Worktree an" || fail "Prüfungen legen keinen Worktree an"

# --- Start ---
run $SPAWN rust-developer 0099 "$FEATURE"
assert_eq "Start: Exit 0" 0 "$RC"
assert_contains "Start: Meldung" "rust-developer-0099 bereit in $TH_WT/rust-developer" "$OUT"
assert_eq "Start: ein hc run mit Name, Cwd und Labels" 1 \
  "$(calls "^run --bg --name rust-developer-0099 --cwd $TH_WT/rust-developer --label ticket=0099 --label role=rust-developer --no-parent --no-notify claude --agent rust-developer --name rust-developer-0099")"
[ -d "$TH_WT/rust-developer" ] && ok "Start: eigener Worktree" || fail "Start: eigener Worktree"
run $SPAWN tech-lead 0099 "$FEATURE"
assert_eq "geteilte Rolle: Exit 0" 0 "$RC"
assert_contains "geteilte Rolle arbeitet im Integrations-Worktree" "bereit in $TH_WT/integration" "$OUT"

# --- läuft bereits / Instanzen ---
run $SPAWN rust-developer 0099 "$FEATURE"
assert_eq "läuft bereits: Exit 1" 1 "$RC"
assert_contains "Hinweis auf --instance" "--instance" "$OUT"
run $SPAWN --instance rust-developer 0099 "$FEATURE"
assert_eq "--instance: Exit 0" 0 "$RC"
assert_contains "--instance: kleinste freie Instanz ist 2" "rust-developer-0099-2 bereit in $TH_WT/rust-developer-2" "$OUT"

# --- Obergrenze (3 aktiv, Orchestrator zählt nicht) ---
th_terminals '[{"address":"t1","name":"orchestrator"},{"name":"architect-0099"},{"name":"merger-0099"},{"name":"tech-lead-0099"}]'
run $SPAWN test-engineer 0099 "$FEATURE"
assert_eq "Obergrenze erreicht: Exit 1" 1 "$RC"
assert_contains "Meldung nennt die Obergrenze" "Obergrenze" "$OUT"
assert_eq "Obergrenze: kein hc run" 0 "$(calls '^run')"
th_terminals '[{"address":"t1","name":"orchestrator"},{"name":"architect-0099"},{"name":"merger-0099"}]'
run $SPAWN test-engineer 0099 "$FEATURE"
assert_eq "eine unter der Obergrenze: Exit 0" 0 "$RC"

# --- Codex startet ohne Rollen-Argumente ---
th_terminals '[]'
run $SPAWN --kind codex architect 0099 "$FEATURE"
assert_eq "--kind codex: Exit 0" 0 "$RC"
assert_eq "--kind codex: codex ohne Argumente" 1 "$(calls ' --no-notify codex$')"

# --- Fehler nach der Änderung: Exit 3 ---
th_terminals '[]'
echo blocked >"$FAKE_DIR/run_state"
run $SPAWN release-engineer 0099 "$FEATURE"
assert_eq "blockierter Agent: Exit 3" 3 "$RC"
assert_contains "Meldung: nicht bereit" "nicht bereit" "$OUT"
rm -f "$FAKE_DIR/run_state"
: >"$FAKE_DIR/run_fail"
th_terminals '[]'
run $SPAWN product-owner 0099 "$FEATURE"
assert_eq "hc run scheitert: Exit 3" 3 "$RC"
rm -f "$FAKE_DIR/run_fail"

# --- close ---
th_terminals '[{"address":"t1","name":"orchestrator"},{"address":"t5","name":"merger-0099"}]'
run $SPAWN close merger-0099
assert_eq "close: Exit 0" 0 "$RC"
assert_eq "close: hc kill" 1 "$(calls '^kill merger-0099$')"
run $SPAWN close merger-0099
assert_eq "close: keine laufende Session: Exit 1" 1 "$RC"
run $SPAWN close orchestrator
assert_eq "close orchestrator verweigert: Exit 1" 1 "$RC"
assert_eq "close orchestrator: kein hc kill" 0 "$(calls '^kill')"
run env -u HC_TERMINAL_ID $SPAWN close merger-0099
assert_eq "close außerhalb von herdr-core: Exit 1" 1 "$RC"

th_summary
