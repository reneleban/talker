#!/bin/sh
# T11: Bash-Wächter des tech-lead (AK-15). Reiner stdin→Exit-Code-Test, ohne herdr.
. "$(dirname "$0")/lib.sh"

HOOK="$TH_SRC/.claude/hooks/say-only.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/say-only.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

# hook <name> <erwarteter Exit> <command>: JSON-Eingabe wie bei einem PreToolUse-Hook
hook() {
  printf '%s' "$3" | jq -Rs '{tool_name: "Bash", tool_input: {command: .}}' >"$TMP/in.json"
  sh "$HOOK" <"$TMP/in.json" >"$TMP/out" 2>"$TMP/err"
  rc=$?
  assert_eq "$1" "$2" "$rc"
  if [ "$2" = 2 ]; then
    if [ -s "$TMP/err" ]; then ok "$1: Begründung auf stderr"; else fail "$1: Begründung auf stderr"; fi
  fi
}

# --- erlaubt ---
hook "erlaubt: say.sh an orchestrator" 0 "sh scripts/team/say.sh orchestrator 'fertig tech-lead 0043: kein Commit — 19 Tasks'"
hook "erlaubt: Ziel mit Ziffern, - und _" 0 "sh scripts/team/say.sh rust-developer-0043-2 'ok'"
hook "erlaubt: Ziel mit 32 Zeichen" 0 "sh scripts/team/say.sh aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 'ok'"
hook "erlaubt: \$( ) in einfachen Anführungszeichen ist Text, keine Expansion" 0 "sh scripts/team/say.sh orchestrator 'kein \$(rm -rf x) hier'"

# --- verboten ---
hook "verboten: ; rm hinter dem Aufruf" 2 "sh scripts/team/say.sh orchestrator 'x'; rm -rf /"
hook "verboten: && hinter dem Aufruf" 2 "sh scripts/team/say.sh orchestrator 'x' && echo hi"
hook "verboten: | hinter dem Aufruf" 2 "sh scripts/team/say.sh orchestrator 'x' | cat"
hook "verboten: Umleitung hinter dem Aufruf" 2 "sh scripts/team/say.sh orchestrator 'x' > /tmp/datei"
hook "verboten: \$( ) im Ziel" 2 "sh scripts/team/say.sh \$(echo orchestrator) 'x'"
hook "verboten: \$( ) ohne Anführungszeichen im Text" 2 "sh scripts/team/say.sh orchestrator \$(whoami)"
hook "verboten: zweites ' im Text" 2 "sh scripts/team/say.sh orchestrator 'a'b'"
hook "verboten: ' schließen und Befehl anhängen" 2 "sh scripts/team/say.sh orchestrator 'a'; rm -rf x; echo 'b'"
hook "verboten: zweite Zeile" 2 "sh scripts/team/say.sh orchestrator 'x'
rm -rf /"
hook "verboten: anderes Skript" 2 "sh scripts/team/other.sh orchestrator 'x'"
hook "verboten: bash statt sh" 2 "bash scripts/team/say.sh orchestrator 'x'"
hook "verboten: anderer Pfad" 2 "sh ../scripts/team/say.sh orchestrator 'x'"
hook "verboten: führendes Leerzeichen" 2 " sh scripts/team/say.sh orchestrator 'x'"
hook "verboten: leerer Text" 2 "sh scripts/team/say.sh orchestrator ''"
hook "verboten: Text in doppelten Anführungszeichen" 2 "sh scripts/team/say.sh orchestrator \"x\""
hook "verboten: Ziel mit Großbuchstaben" 2 "sh scripts/team/say.sh Orchestrator 'x'"
hook "verboten: Ziel mit 33 Zeichen" 2 "sh scripts/team/say.sh aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 'x'"
hook "verboten: Ziel fehlt" 2 "sh scripts/team/say.sh 'x'"
hook "verboten: beliebiger Befehl" 2 "rm -rf /"
hook "verboten: leerer Befehl" 2 ""

# --- kaputte Eingabe → Exit 2 ---
printf '%s' '{"tool_input":{}}' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: command fehlt im JSON" 2 $?
printf '%s' 'kein json' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: Eingabe ist kein JSON" 2 $?
printf '' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: leere Eingabe" 2 $?

# --- fail-closed ohne jq ---
mkdir -p "$TMP/leer"
printf '%s' '{"tool_input":{"command":"sh scripts/team/say.sh orchestrator '"'x'"'"}}' >"$TMP/ok.json"
PATH="$TMP/leer" /bin/sh "$HOOK" <"$TMP/ok.json" >/dev/null 2>"$TMP/err"
assert_eq "verboten: jq fehlt im PATH (fail-closed)" 2 $?
assert_contains "jq fehlt: Begründung nennt jq" jq "$(cat "$TMP/err")"

th_summary
