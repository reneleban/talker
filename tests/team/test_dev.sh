#!/bin/sh
# T2/T3: make dev — Fehlerpfad PATH (AK-4), Session + Orchestrator headless (AK-1), Server überlebt (A1),
# idempotent (AK-2), Neustart des Orchestrators (AK-3).
. "$(dirname "$0")/lib.sh"

th_begin
DEV="$TH_REPO/scripts/team/dev.sh"

# --- AK-4: fehlendes Programm im PATH → Exit 1, Meldung nennt es ---
BIN="$TH_TMP/bin"
mkdir -p "$BIN/nur-herdr" "$BIN/nur-claude" "$BIN/ohne-jq"
ln -s "$(command -v herdr)" "$BIN/nur-herdr/herdr"
ln -s "$(command -v claude)" "$BIN/nur-claude/claude"
ln -s "$(command -v herdr)" "$BIN/ohne-jq/herdr"
ln -s "$(command -v claude)" "$BIN/ohne-jq/claude"
ln -s "$(command -v git)" "$BIN/ohne-jq/git"
BASE=/usr/bin:/bin
# /usr/bin enthält auf neuem macOS selbst ein jq: für den jq-Fall nur ein PATH mit herdr, claude, git

out=$(cd "$TH_REPO" && PATH="$BIN/nur-claude:$BASE" TEAM_SESSION=$TS sh "$DEV" 2>&1 </dev/null); rc=$?
assert_eq "AK-4 ohne herdr: Exit 1" 1 "$rc"
assert_contains "AK-4 ohne herdr: Meldung nennt herdr" "herdr fehlt im PATH" "$out"

out=$(cd "$TH_REPO" && PATH="$BIN/nur-herdr:$BASE" TEAM_SESSION=$TS sh "$DEV" 2>&1 </dev/null); rc=$?
assert_eq "AK-4 ohne claude: Exit 1" 1 "$rc"
assert_contains "AK-4 ohne claude: Meldung nennt claude" "claude fehlt im PATH" "$out"
out=$(cd "$TH_REPO" && PATH="$BIN/ohne-jq" TEAM_SESSION=$TS /bin/sh "$DEV" 2>&1 </dev/null); rc=$?
assert_eq "AK-4 ohne jq: Exit 1" 1 "$rc"
assert_contains "AK-4 ohne jq: Meldung nennt jq" "jq fehlt im PATH" "$out"
if th_session_exists; then fail "AK-4: Fehlerpfad startet keine Session"; else ok "AK-4: Fehlerpfad startet keine Session"; fi

# --- AK-1: Session + Orchestrator, ohne TTY (stdout in Datei) → attach scheitert erwartet ---
(cd "$TH_REPO" && env -u HERDR_ENV -u HERDR_SESSION -u HERDR_SOCKET_PATH -u HERDR_PANE_ID -u HERDR_TAB_ID -u HERDR_WORKSPACE_ID TEAM_SESSION=$TS sh "$DEV" >"$TH_TMP/dev1.out" 2>&1 </dev/null); rc=$?
assert_eq "AK-1 ohne TTY endet attach mit Exit ≠ 0" 1 "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
grep -E '^dev:|^\{"error' "$TH_TMP/dev1.out" | sed 's/^/       /' || true
assert_contains "AK-1 attach scheitert am Terminal" "usable terminal" "$(cat "$TH_TMP/dev1.out")"

if th_session_exists; then ok "AK-1 Server läuft nach dem Ende des Skripts (A1)"; else fail "AK-1 Server läuft nach dem Ende des Skripts (A1)"; fi
ORCH='.result.agents[] | select(.name == "orchestrator")'
assert_eq "AK-1 genau ein Agent orchestrator" 1 \
  "$(th_h agent list | jq '[.result.agents[] | select(.name == "orchestrator")] | length')"
assert_eq "AK-1 orchestrator ist ein Claude-Agent" claude "$(th_h agent list | jq -r "$ORCH | .agent")"
FIRST_TAB=$(th_h tab list | jq -r '.result.tabs | sort_by(.number) | .[0].tab_id')
assert_eq "AK-1 orchestrator liegt im ersten Tab" "$FIRST_TAB" "$(th_h agent list | jq -r "$ORCH | .tab_id")"
CWD=$(th_h agent list | jq -r "$ORCH | .cwd")
assert_eq "AK-1 Arbeitsort des Orchestrators ist das Haupt-Repo" "$TH_REPO" "$(cd "$CWD" && pwd -P)"

# B9: Panes erben die Server-Umgebung — ohne CLAUDE*-Variablen, auch wenn der Aufrufer welche hat
TH_PANE=$(th_h pane split "$(th_h agent list | jq -r "$ORCH | .pane_id")" \
  --direction right --cwd "$TH_REPO" --no-focus | jq -r '.result.pane.pane_id')
inpane 'env | grep -c "^CLAUDE" || true'
assert_eq "AK-1 Pane-Umgebung ohne CLAUDE*-Variablen (B9)" 0 "$INPANE_OUT"


# --- T3: idempotent ---
run_dev() { (cd "$TH_REPO" && TEAM_SESSION=$TS sh "$DEV" >"$TH_TMP/dev.$1.out" 2>&1 </dev/null) || true
  grep '^dev:' "$TH_TMP/dev.$1.out" | sed 's/^/       /' || true; }
orch_count() { th_h agent list | jq '[.result.agents[] | select(.name == "orchestrator")] | length'; }
orch_pane() { th_h agent list | jq -r "$ORCH | .pane_id"; }
orch_tab() { th_h agent list | jq -r "$ORCH | .tab_id"; }
first_tab() { th_h tab list | jq -r '.result.tabs | sort_by(.number) | .[0].tab_id'; }
pane_count() { th_h pane list | jq '.result.panes | length'; }

PANES=$(pane_count)
PANE1=$(orch_pane)
run_dev 2
assert_eq "AK-2 zweiter Lauf: weiterhin ein orchestrator" 1 "$(orch_count)"
assert_eq "AK-2 zweiter Lauf: gleiche Pane" "$PANE1" "$(orch_pane)"
assert_eq "AK-2 zweiter Lauf: keine neue Pane" "$PANES" "$(pane_count)"
assert_eq "AK-2 zweiter Lauf: kein zweiter Arbeitsbereich" 1 "$(th_h workspace list | jq '.result.workspaces | length')"

# A5: Claude ohne herdr-Namen in der ersten Pane → bekommt den Namen zurück, kein zweiter Agent
th_trust orchestrator
th_h agent rename orchestrator --clear >/dev/null
assert_eq "A5 Vorbedingung: orchestrator hat keinen Namen" 0 "$(orch_count)"
run_dev 3
assert_eq "A5 unbenannter Claude wird zu orchestrator" 1 "$(orch_count)"
assert_eq "A5 gleiche Pane, keine neue" "$PANE1" "$(orch_pane)"
assert_eq "A5 keine neue Pane" "$PANES" "$(pane_count)"

# AK-3: orchestrator fehlt → Neustart im ersten Tab
th_h pane close "$(orch_pane)" >/dev/null
assert_eq "AK-3 Vorbedingung: orchestrator fehlt" 0 "$(orch_count)"
run_dev 4
assert_eq "AK-3 orchestrator läuft wieder" 1 "$(orch_count)"
assert_eq "AK-3 im ersten Tab" "$(first_tab)" "$(orch_tab)"

# Grenzfall: erste Pane des ersten Tabs belegt (keine Shell) → Split rechts
th_h pane split "$(orch_pane)" --direction right --cwd "$TH_REPO" --no-focus >/dev/null
th_h pane close "$(orch_pane)" >/dev/null
BUSY=$(th_h pane list | jq -r --arg t "$(first_tab)" '[.result.panes[] | select(.tab_id == $t)] | .[0].pane_id')
assert_eq "Grenzfall belegte Pane: Vorbedingung, erster Tab hat eine Pane" 1 "$([ -n "$BUSY" ] && [ "$BUSY" != null ] && echo 1 || echo 0)"
th_h pane run "$BUSY" 'sleep 600' >/dev/null
sleep 1
PANES=$(pane_count)
run_dev 5
assert_eq "Grenzfall belegte Pane: orchestrator läuft" 1 "$(orch_count)"
assert_eq "Grenzfall belegte Pane: eine Pane mehr" "$((PANES + 1))" "$(pane_count)"
assert_eq "Grenzfall belegte Pane: nicht in der belegten Pane" 1 "$([ "$(orch_pane)" != "$BUSY" ] && echo 1 || echo 0)"
assert_eq "Grenzfall belegte Pane: im ersten Tab" "$(first_tab)" "$(orch_tab)"

th_summary
