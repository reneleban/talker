#!/bin/sh
# say.sh gegen Fake-hc: Aufruffehler (Exit 2), Umgebung/Absender/Ziel (Exit 1), Zustellung mit Kopfzeile, Wiederholungen.
. "$(dirname "$0")/lib.sh"

th_begin
cd "$TH_REPO" || exit 1
SAY='sh scripts/team/say.sh'
export SAY_RETRY_TIMEOUT_S=1
th_terminals '[{"address":"t1","name":"sender"},{"address":"t2","name":"empfaenger"}]'

run() { : >"$LOG"; OUT=$("$@" 2>&1); RC=$?; }

# --- Aufruffehler → Exit 2, hc wird nicht aufgerufen ---
run $SAY;                                     assert_eq "kein Argument: Exit 2" 2 "$RC"
run $SAY ziel;                                assert_eq "ein Argument: Exit 2" 2 "$RC"
run $SAY ziel text extra;                     assert_eq "drei Argumente: Exit 2" 2 "$RC"
run $SAY Foo text;                            assert_eq "Ziel mit Großbuchstaben: Exit 2" 2 "$RC"
run $SAY aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa text; assert_eq "Ziel mit 33 Zeichen: Exit 2" 2 "$RC"
run $SAY aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa text;  assert_eq "Ziel mit 32 Zeichen ist gültig (weiter bis Exit 1)" 1 "$RC"
run $SAY ziel '';                             assert_eq "leerer Text: Exit 2" 2 "$RC"
run $SAY Foo text
assert_eq "Aufruffehler rufen hc nicht auf" 0 "$(calls .)"

# --- Umgebung ---
run env -u HC_TERMINAL_ID $SAY empfaenger text
assert_eq "HC_TERMINAL_ID fehlt: Exit 1" 1 "$RC"
assert_eq "ohne Umgebung kein hc-Aufruf" 0 "$(calls .)"

# --- Absender = Name des eigenen Terminals ---
run env HC_TERMINAL_ID=t9 $SAY empfaenger text
assert_eq "eigenes Terminal unbekannt: Exit 1" 1 "$RC"
th_terminals '[{"address":"t1"},{"address":"t2","name":"empfaenger"}]'
run $SAY empfaenger text
assert_eq "eigenes Terminal ohne Namen: Exit 1" 1 "$RC"
assert_contains "Meldung nennt den fehlenden Absender" "Absender" "$OUT"
assert_eq "ohne Absender wird nichts gesendet" 0 "$(calls '^send')"
th_terminals '[{"address":"t1","name":"sender"},{"address":"t2","name":"empfaenger"}]'

# --- unbekanntes Ziel ---
run $SAY nirgendwo text
assert_eq "unbekanntes Ziel: Exit 1" 1 "$RC"
assert_contains "Meldung nennt das Ziel" nirgendwo "$OUT"
assert_eq "unbekanntes Ziel: kein send" 0 "$(calls '^send')"

# --- Zustellung mit Kopfzeile ---
run $SAY empfaenger 'Antworte nur mit OK.'
assert_eq "Zustellung: Exit 0" 0 "$RC"
assert_eq "genau ein send" 1 "$(calls '^send')"
assert_eq "send an den Empfänger mit Kopfzeile" 1 "$(calls '^send empfaenger \[talker von sender\] Antworte nur mit OK\.$')"
assert_eq "kein Warten bei erster Zustellung" 0 "$(calls '^wait')"

# --- nicht zustellbar: 3 Versuche, dazwischen gewartet, dann Exit 1 ---
: >"$FAKE_DIR/send_fail"
run $SAY empfaenger text
assert_eq "nicht zustellbar: Exit 1" 1 "$RC"
assert_contains "Meldung nennt 3 Versuche" "3 Versuchen" "$OUT"
assert_eq "genau 3 Versuche" 3 "$(calls '^send')"
assert_eq "zwischen den Versuchen wird gewartet" 2 "$(calls '^wait empfaenger --timeout 1$')"

th_summary
