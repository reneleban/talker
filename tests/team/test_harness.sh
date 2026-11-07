#!/bin/sh
# T1: Harness-Smoke — Session wird angelegt, Befehle laufen in der Pane, `talker` bleibt unverändert.
. "$(dirname "$0")/lib.sh"

others() { herdr session list --json | jq -c --arg n "$TS" '[.sessions[] | select(.name != $n)]'; }

BEFORE=$(others)
th_begin
th_server

assert_eq "Wegwerf-Repo hat Commit" 1 "$(git -C "$TH_REPO" rev-list --count HEAD)"
inpane 'echo hallo; echo fehler >&2'
assert_eq "inpane liefert Exit 0" 0 "$INPANE_RC"
assert_contains "inpane liefert stdout" hallo "$INPANE_OUT"
assert_contains "inpane liefert stderr" fehler "$INPANE_OUT"
inpane 'exit_code() { return 7; }; exit_code'
assert_eq "inpane liefert Exit-Code des Befehls" 7 "$INPANE_RC"
inpane 'echo "$HERDR_ENV:$HERDR_SESSION"'
assert_eq "Pane läuft in der Test-Session" "1:$TS" "$INPANE_OUT"
inpane 'env | grep -c "^CLAUDE" || true'
assert_eq "Pane erbt keine CLAUDE*-Variablen" 0 "$INPANE_OUT"
assert_eq "andere Sessions während des Tests unverändert" "$BEFORE" "$(others)"

th_teardown
if th_session_exists; then fail "Test-Session nach Teardown entfernt"; else ok "Test-Session nach Teardown entfernt"; fi
assert_eq "andere Sessions nach Teardown unverändert" "$BEFORE" "$(others)"
th_summary
