#!/bin/sh
# T2: make dev — Fehlerpfad PATH (AK-4), Session + Orchestrator headless (AK-1), Server überlebt (A1).
. "$(dirname "$0")/lib.sh"

th_begin
DEV="$TH_REPO/scripts/team/dev.sh"

# --- AK-4: fehlendes Programm im PATH → Exit 1, Meldung nennt es ---
BIN="$TH_TMP/bin"
mkdir -p "$BIN/nur-herdr" "$BIN/nur-claude"
ln -s "$(command -v herdr)" "$BIN/nur-herdr/herdr"
ln -s "$(command -v claude)" "$BIN/nur-claude/claude"
BASE=/usr/bin:/bin

out=$(cd "$TH_REPO" && PATH="$BIN/nur-claude:$BASE" TEAM_SESSION=$TS sh "$DEV" 2>&1 </dev/null); rc=$?
assert_eq "AK-4 ohne herdr: Exit 1" 1 "$rc"
assert_contains "AK-4 ohne herdr: Meldung nennt herdr" "herdr fehlt im PATH" "$out"

out=$(cd "$TH_REPO" && PATH="$BIN/nur-herdr:$BASE" TEAM_SESSION=$TS sh "$DEV" 2>&1 </dev/null); rc=$?
assert_eq "AK-4 ohne claude: Exit 1" 1 "$rc"
assert_contains "AK-4 ohne claude: Meldung nennt claude" "claude fehlt im PATH" "$out"
if th_session_exists; then fail "AK-4: Fehlerpfad startet keine Session"; else ok "AK-4: Fehlerpfad startet keine Session"; fi

# --- AK-1: Session + Orchestrator, ohne TTY (stdout in Datei) → attach scheitert erwartet ---
(cd "$TH_REPO" && TEAM_SESSION=$TS sh "$DEV" >"$TH_TMP/dev1.out" 2>&1 </dev/null); rc=$?
assert_eq "AK-1 ohne TTY endet attach mit Exit ≠ 0" 1 "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
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

th_summary
