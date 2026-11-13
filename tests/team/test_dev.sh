#!/bin/sh
# dev.sh gegen Fake-hc: Server sicherstellen, Orchestrator genau einmal starten, verbinden.
. "$(dirname "$0")/lib.sh"

th_begin
cd "$TH_REPO" || exit 1
DEV='sh scripts/team/dev.sh'

# claude im PATH des Tests (nur Existenzprüfung)
printf '#!/bin/sh\n' >"$FAKE_DIR/bin/claude"
chmod +x "$FAKE_DIR/bin/claude"

run() { : >"$LOG"; OUT=$("$@" 2>&1); RC=$?; }

# --- frischer Start: Orchestrator wird gestartet und verbunden ---
th_terminals '[]'
run $DEV
assert_eq "Start: Exit 0" 0 "$RC"
assert_eq "Start: ein hc run für den Orchestrator im Haupt-Checkout" 1 \
  "$(calls "^run --bg --name orchestrator --cwd $TH_REPO --no-parent --keep claude --name orchestrator$")"
assert_eq "Start: verbindet mit dem Orchestrator" 1 "$(calls '^attach orchestrator$')"

# --- Orchestrator läuft schon: nichts doppelt starten ---
run $DEV
assert_eq "Wiederholung: Exit 0" 0 "$RC"
assert_eq "Wiederholung: kein weiterer hc run" 0 "$(calls '^run')"
assert_eq "Wiederholung: verbindet" 1 "$(calls '^attach orchestrator$')"

# --- Server nicht erreichbar: wird gestartet ---
: >"$FAKE_DIR/server_down"
run $DEV
assert_eq "Server down: Exit 0" 0 "$RC"
assert_eq "Server down: hc server start" 1 "$(calls '^server start$')"

# --- Start scheitert: Abbruch mit Meldung, kein attach ---
th_terminals '[]'
: >"$FAKE_DIR/run_fail"
run $DEV
assert_eq "hc run scheitert: Exit 1" 1 "$RC"
assert_contains "Meldung nennt den Orchestrator" orchestrator "$OUT"
assert_eq "hc run scheitert: kein attach" 0 "$(calls '^attach')"
rm -f "$FAKE_DIR/run_fail"

# --- Voraussetzungen ---
mkdir -p "$TH_TMP/leer"
OUT=$(PATH="$TH_TMP/leer" /bin/sh scripts/team/dev.sh 2>&1); RC=$?
assert_eq "hc fehlt im PATH: Exit 1" 1 "$RC"
assert_contains "Meldung nennt hc" "hc fehlt" "$OUT"

th_summary
