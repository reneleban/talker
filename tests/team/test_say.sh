#!/bin/sh
# T4: say.sh — Aufruffehler (Exit 2), Umgebung/Absender/Ziel (Exit 1), Zustellung mit Kopfzeile (AK-12),
# blockiertes Ziel mit Wiederholungen (AK-13).
. "$(dirname "$0")/lib.sh"

th_begin
th_server

# herdr-Shim protokolliert jeden Aufruf (prüft u. a. „kein --wait" und die Zahl der Versuche)
SHIM="$TH_TMP/shim"
mkdir -p "$SHIM"
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >>"%s/herdr.log"\nexec "%s" "$@"\n' "$TH_TMP" "$(command -v herdr)" >"$SHIM/herdr"
chmod +x "$SHIM/herdr"
LOG="$TH_TMP/herdr.log"
calls() { grep -c "$1" "$LOG" 2>/dev/null || true; }

SAY='sh scripts/team/say.sh'
run() { : >"$LOG"; inpane "PATH=$SHIM:\$PATH HERDR_LOG=$LOG $*"; }

# --- Aufruffehler → Exit 2 (vor jeder Umgebungsprüfung) ---
run "$SAY"
assert_eq "AK-12 kein Argument: Exit 2" 2 "$INPANE_RC"
run "$SAY ziel"
assert_eq "AK-12 ein Argument: Exit 2" 2 "$INPANE_RC"
run "$SAY ziel text extra"
assert_eq "AK-12 drei Argumente: Exit 2" 2 "$INPANE_RC"
run "$SAY Foo 'text'"
assert_eq "AK-12 Ziel mit Großbuchstaben: Exit 2" 2 "$INPANE_RC"
run "$SAY aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 'text'"
assert_eq "AK-12 Ziel mit 33 Zeichen: Exit 2" 2 "$INPANE_RC"
run "$SAY aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 'text'"
assert_eq "AK-12 Ziel mit 32 Zeichen ist gültig (weiter bis Exit 1)" 1 "$INPANE_RC"
run "$SAY ziel ''"
assert_eq "AK-12 leerer Text: Exit 2" 2 "$INPANE_RC"
assert_eq "AK-12 Aufruffehler rufen herdr nicht auf" 0 "$(calls prompt)"

# --- Umgebung ---
run "HERDR_ENV= $SAY ziel 'text'"
assert_eq "AK-12 HERDR_ENV leer: Exit 1" 1 "$INPANE_RC"
run "env -u HERDR_PANE_ID $SAY ziel 'text'"
assert_eq "AK-12 HERDR_PANE_ID fehlt: Exit 1" 1 "$INPANE_RC"

# --- Absender = Agent der eigenen Pane ---
# Der blockierte Agent muss der erste Claude sein: Nur der erste zeigt im frischen Ordner den
# Vertrauensdialog (geprüft: ein später gestarteter Claude ist sofort idle).
mkdir -p "$TH_TMP/frisch"
TH_AGENT_CWD=$TH_TMP/frisch th_agent blockiert
assert_eq "AK-13 Vorbedingung: Ziel ist blocked" blocked "$(th_h agent get blockiert | jq -r '.result.agent.agent_status')"
th_agent sender
SENDER_PANE=$TH_AGENT_PANE
run "$SAY ziel 'text'"
assert_eq "AK-12 Pane ohne Agent: kein Absender, Exit 1" 1 "$INPANE_RC"
assert_contains "AK-12 Meldung nennt den fehlenden Absender" "Absender" "$INPANE_OUT"
assert_eq "AK-12 ohne Absender wird nichts gesendet" 0 "$(calls 'agent prompt')"

# --- unbekanntes Ziel → sofort Exit 1 ---
run "HERDR_PANE_ID=$SENDER_PANE $SAY nirgendwo 'text'"
assert_eq "AK-13 unbekanntes Ziel: Exit 1" 1 "$INPANE_RC"
assert_contains "AK-13 Meldung nennt das Ziel" nirgendwo "$INPANE_OUT"
assert_eq "AK-13 unbekanntes Ziel: kein prompt-Aufruf" 0 "$(calls 'agent prompt')"

# --- Zustellung mit Kopfzeile, ohne --wait ---
th_agent empfaenger trust
run "HERDR_PANE_ID=$SENDER_PANE $SAY empfaenger 'Antworte nur mit dem Wort OK.'"
assert_eq "AK-12 Zustellung: Exit 0" 0 "$INPANE_RC"
assert_eq "AK-12 ein prompt-Aufruf" 1 "$(calls 'agent prompt')"
assert_eq "AK-12 prompt ohne --wait" 0 "$(grep 'agent prompt' "$LOG" | grep -c -e '--wait' || true)"
i=0
while [ $i -lt 40 ]; do
  SEEN=$(th_h agent read empfaenger --source recent-unwrapped --lines 100 | tr -s ' \n' ' ')
  case $SEEN in *"[talker von sender] Antworte nur mit dem Wort OK."*) break ;; esac
  i=$((i + 1)); sleep 0.5
done
assert_contains "AK-12 Empfänger sieht Kopfzeile mit Absender-Name" "[talker von sender] Antworte nur mit dem Wort OK." "$SEEN"

# --- blockiertes Ziel (Vertrauensdialog): 3 Versuche, dann Exit 1 ---
run "SAY_RETRY_TIMEOUT_MS=500 HERDR_PANE_ID=$SENDER_PANE $SAY blockiert 'text'"
assert_eq "AK-13 blockiertes Ziel: Exit 1" 1 "$INPANE_RC"
assert_contains "AK-13 Meldung" "blockiert nach 3 Versuchen" "$INPANE_OUT"
assert_eq "AK-13 genau 3 Versuche" 3 "$(calls 'agent prompt')"
assert_eq "AK-13 zwischen den Versuchen wird gewartet" 2 "$(calls 'agent wait')"
assert_eq "AK-13 Wartezeit kommt aus SAY_RETRY_TIMEOUT_MS" 2 "$(grep 'agent wait' "$LOG" | grep -c -e '--timeout 500' || true)"

th_summary
