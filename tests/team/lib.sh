#!/bin/sh
# Test-Harness für die Team-Skripte (Tickets 0043, 0051). Wird von den test_*.sh gesourct.
#
# Die Skripte sprechen nur mit `hc` (herdr-core). Die Tests ersetzen `hc` durch einen Fake im PATH:
# kein Server, kein Claude, kein Netz, nichts vom echten Team wird angefasst.
#
#   th_begin           Wegwerf-Repo ($TH_REPO, $TH_WT), Fake-hc im PATH, Teardown-Trap
#   th_terminals '<json-array>'   Terminals des Fake-hc setzen, z. B. '[{"address":"t1","name":"orchestrator"}]'
#   calls <muster>     Zahl der protokollierten hc-Aufrufe, die das Muster (grep -E) treffen
#   ok / fail / assert_* / th_summary
set -u

TH_SRC=$(cd "$(dirname "$0")/../.." && pwd -P)
TH_PASS=0
TH_FAIL=0
TH_TMP=""
TH_REPO=""
TH_WT=""
FAKE_DIR=""
LOG=""

ROLES="product-owner architect tech-lead rust-developer test-engineer code-reviewer security-auditor performance-engineer merger release-engineer"

ok() { TH_PASS=$((TH_PASS + 1)); echo "  ok   $1"; }
fail() { TH_FAIL=$((TH_FAIL + 1)); echo "  FAIL $1"; }

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
  [ -z "$TH_TMP" ] || rm -rf "$TH_TMP"
  TH_TMP=""
}

th_begin() {
  command -v jq >/dev/null || { echo "harness: jq fehlt im PATH" >&2; exit 1; }
  TH_TMP=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/talker-team-test.XXXXXX")" && pwd -P)
  trap 'th_teardown' EXIT
  trap 'exit 130' INT TERM
  TH_REPO="$TH_TMP/talker"
  TH_WT="$TH_TMP/talker-wt"
  FAKE_DIR="$TH_TMP/fake"
  LOG="$FAKE_DIR/log"
  mkdir -p "$FAKE_DIR/bin"
  : >"$LOG"
  th_fake_hc
  PATH="$FAKE_DIR/bin:$PATH"
  export PATH FAKE_DIR
  HC_TERMINAL_ID=t1
  export HC_TERMINAL_ID
  th_terminals '[]'
  th_make_repo
}

th_terminals() { printf '{"terminals":%s}\n' "$1" >"$FAKE_DIR/ls.json"; }

calls() { grep -Ec -e "$1" "$LOG" 2>/dev/null || true; }

# Fake-hc: protokolliert jeden Aufruf; Zustand liegt in $FAKE_DIR/ls.json.
# Schalter über Dateien: send_fail (send scheitert), run_fail (run scheitert), server_down (server status scheitert),
# run_state (Status, den ein neues Terminal meldet, Default idle).
th_fake_hc() {
  cat >"$FAKE_DIR/bin/hc" <<'FAKE'
#!/bin/sh
echo "$*" >>"$FAKE_DIR/log"
LS="$FAKE_DIR/ls.json"
cmd=$1; shift
case $cmd in
  ls) cat "$LS" ;;
  get) jq -c --arg t "$1" '[.terminals[] | select(.address == $t or .name == $t)] | .[0] // empty' "$LS" ;;
  send) [ ! -f "$FAKE_DIR/send_fail" ] || { echo "terminal is blocked" >&2; exit 1; } ;;
  wait) exit 0 ;;
  run)
    [ ! -f "$FAKE_DIR/run_fail" ] || { echo "run failed" >&2; exit 1; }
    name=""
    while [ $# -gt 0 ]; do case $1 in --name) name=$2; break ;; *) shift ;; esac; done
    state=$(cat "$FAKE_DIR/run_state" 2>/dev/null || echo idle)
    jq --arg n "$name" --arg s "$state" '.terminals += [{address: ("t" + $n), name: $n, status: {state: $s}}]' "$LS" >"$LS.new" \
      && mv "$LS.new" "$LS" ;;
  kill) jq --arg t "$1" '.terminals |= map(select(.address != $t and .name != $t))' "$LS" >"$LS.new" && mv "$LS.new" "$LS" ;;
  server)
    case $1 in
      status) [ ! -f "$FAKE_DIR/server_down" ] ;;
      start) rm -f "$FAKE_DIR/server_down" ;;
    esac ;;
  attach) ;;
esac
FAKE
  chmod +x "$FAKE_DIR/bin/hc"
}

# Wegwerf-Repo im Layout des echten Projekts: <tmp>/talker + <tmp>/talker-wt
th_make_repo() {
  mkdir -p "$TH_REPO/.claude/agents" "$TH_REPO/.claude/skills/spawn-worker" "$TH_REPO/scripts/team" "$TH_WT"
  for r in $ROLES; do
    printf -- '---\nname: %s\ndescription: Wegwerf-Rolle für Skript-Tests\n---\nAntworte knapp.\n' "$r" \
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
