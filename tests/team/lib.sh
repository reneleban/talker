#!/bin/sh
# Test-Harness für die Team-Skripte (Ticket 0043). Wird von den test_*.sh gesourct.
#
# Sicherheitsnetz (R6/B11): Die herdr-Session des Menschen (`talker`, `default`) wird nie
# angefasst. Alles läuft in einer eigenen Session (Default `talker-test`), die der Test
# selbst anlegt und am Ende per `session stop` + `session delete` wieder entfernt. Existiert
# sie schon, bricht der Test ab, statt eine fremde Session zu übernehmen.
#
#   th_begin           Guard + Wegwerf-Repo ($TH_REPO, $TH_WT) + Teardown-Trap
#   th_server          Server headless starten, Arbeitsbereich + Shell-Pane ($TH_PANE)
#   inpane '<cmd>'     Befehl in $TH_PANE ausführen → $INPANE_RC, $INPANE_OUT (stdout+stderr)
#   th_trust <agent>   Vertrauensdialog beantworten (Agent wird idle)
#   th_h <args…>       herdr gegen die Test-Session
#   ok / fail / assert_* / th_summary
set -u

TS=${TEAM_TEST_SESSION:-talker-test}
TH_SRC=$(cd "$(dirname "$0")/../.." && pwd -P)
TH_PASS=0
TH_FAIL=0
TH_N=0
TH_TMP=""
TH_REPO=""
TH_WT=""
TH_PANE=""

ROLES="product-owner architect tech-lead rust-developer test-engineer code-reviewer security-auditor performance-engineer merger release-engineer"

th_h() { herdr --session "$TS" "$@"; }

th_session_exists() {
  herdr session list --json | jq -e --arg n "$TS" '.sessions[] | select(.name == $n)' >/dev/null 2>&1
}

ok() { TH_PASS=$((TH_PASS + 1)); echo "  ok   $1"; }
fail() { TH_FAIL=$((TH_FAIL + 1)); echo "  FAIL $1"; }
skip() { echo "  skip $1"; }

assert_eq() { # <name> <erwartet> <ist>
  if [ "$2" = "$3" ]; then ok "$1"; else fail "$1 (erwartet '$2', war '$3')"; fi
}
assert_contains() { # <name> <teilstring> <text>
  case $3 in *"$2"*) ok "$1" ;; *) fail "$1 ('$2' fehlt in: $3)" ;; esac
}
assert_not_contains() { # <name> <teilstring> <text>
  case $3 in *"$2"*) fail "$1 ('$2' kommt vor in: $3)" ;; *) ok "$1" ;; esac
}

th_summary() {
  echo "$TH_PASS ok, $TH_FAIL fehlgeschlagen"
  [ "$TH_FAIL" -eq 0 ]
}

th_teardown() {
  if [ -n "$TH_TMP" ] && th_session_exists; then
    herdr session stop "$TS" >/dev/null 2>&1 || true
    herdr session delete "$TS" >/dev/null 2>&1 || true
  fi
  [ -z "$TH_TMP" ] || rm -rf "$TH_TMP"
  TH_TMP=""
}

th_begin() {
  case $TS in talker | default) echo "harness: Session '$TS' ist die des Menschen, Abbruch" >&2; exit 1 ;; esac
  command -v herdr >/dev/null || { echo "harness: herdr fehlt im PATH" >&2; exit 1; }
  command -v jq >/dev/null || { echo "harness: jq fehlt im PATH" >&2; exit 1; }
  if th_session_exists; then
    echo "harness: Session '$TS' existiert schon, nicht angefasst. Abbruch." >&2
    exit 1
  fi
  TH_TMP=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/talker-team-test.XXXXXX")" && pwd -P)
  trap 'th_teardown' EXIT
  trap 'exit 130' INT TERM
  TH_REPO="$TH_TMP/talker"
  TH_WT="$TH_TMP/talker-wt"
  th_make_repo
}

# Wegwerf-Repo im Layout des echten Projekts: <tmp>/talker + <tmp>/talker-wt
th_make_repo() {
  mkdir -p "$TH_REPO/.claude/agents" "$TH_REPO/.claude/skills/spawn-worker" "$TH_REPO/scripts/team" "$TH_WT"
  for r in $ROLES; do
    printf -- '---\nname: %s\ndescription: Wegwerf-Rolle für Skript-Tests\nmodel: haiku\n---\nAntworte knapp.\n' "$r" \
      >"$TH_REPO/.claude/agents/$r.md"
  done
  for f in .claude/skills/spawn-worker/spawn.sh scripts/team/say.sh scripts/team/dev.sh team.env; do
    [ ! -f "$TH_SRC/$f" ] || cp "$TH_SRC/$f" "$TH_REPO/$f"
  done
  (
    cd "$TH_REPO"
    git init -q -b main
    git config user.email test@example.invalid
    git config user.name harness
    git add -A
    git commit -q -m "init" --allow-empty
  )
}

# Server ohne geerbte CLAUDE*-Variablen (B9), Arbeitsbereich im Wegwerf-Repo, Shell-Pane.
th_server() {
  # shellcheck disable=SC2046
  env $(env | sed -n 's/^\(CLAUDE[A-Za-z_]*\)=.*/-u \1/p') \
    nohup herdr --session "$TS" server >/dev/null 2>&1 &
  i=0
  until th_h workspace list >/dev/null 2>&1; do
    i=$((i + 1))
    [ $i -le 100 ] || { echo "harness: Server '$TS' startet nicht" >&2; exit 1; }
    sleep 0.1
  done
  TH_PANE=$(th_h workspace create --cwd "$TH_REPO" --label test --no-focus | jq -r '.result.root_pane.pane_id')
  [ -n "$TH_PANE" ] && [ "$TH_PANE" != null ] || { echo "harness: kein Root-Pane" >&2; exit 1; }
  # feste, prompt-lose Shell: keine Überraschungen durch Nutzer-rc-Dateien
  th_h pane run "$TH_PANE" 'exec env PS1= /bin/sh' >/dev/null
  inpane 'true'
}

# Ordner-Vertrauensdialog eines frischen Wegwerf-Verzeichnisses beantworten (Vorauswahl ist „No, exit").
th_trust() { # <agent>
  th_h agent send-keys "$1" down enter >/dev/null
  th_h agent wait "$1" --until idle --timeout 30000 >/dev/null
}

inpane() { # <befehl> — läuft in der Shell-Pane von $TS; Ausgabe über Datei, Exit-Code über Marker
  TH_N=$((TH_N + 1))
  out="$TH_TMP/out.$TH_N"
  marker="__RC${TH_N}__"
  th_h pane run "$TH_PANE" "{ $1
} >'$out' 2>&1; echo ${marker}=\$?" >/dev/null
  th_h pane wait-output "$TH_PANE" --regex "${marker}=[0-9]+" --source recent-unwrapped --lines 200 \
    --timeout "${INPANE_TIMEOUT_MS:-120000}" >/dev/null 2>&1 || { INPANE_RC=timeout; INPANE_OUT=""; return 0; }
  INPANE_RC=$(th_h pane read "$TH_PANE" --source recent-unwrapped --lines 200 \
    | sed -n "s/.*${marker}=\([0-9][0-9]*\).*/\1/p" | tail -n 1)
  INPANE_OUT=$(cat "$out")
}
