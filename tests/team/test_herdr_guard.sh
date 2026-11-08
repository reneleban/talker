#!/bin/sh
# S2: Bash-Wächter gegen direkte herdr-Aufrufe (alle Rollen). Reiner stdin→Exit-Code-Test, ohne herdr.
. "$(dirname "$0")/lib.sh"

HOOK="$TH_SRC/.claude/hooks/herdr-guard.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/herdr-guard.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

hook() { # <name> <erwarteter Exit> <command>
  printf '%s' "$3" | jq -Rs '{tool_name: "Bash", tool_input: {command: .}}' >"$TMP/in.json"
  sh "$HOOK" <"$TMP/in.json" >"$TMP/out" 2>"$TMP/err"
  rc=$?
  assert_eq "$1" "$2" "$rc"
  if [ "$2" = 2 ]; then
    if [ -s "$TMP/err" ]; then ok "$1: Begründung auf stderr"; else fail "$1: Begründung auf stderr"; fi
  fi
}

# --- erlaubt: lesende herdr-Befehle und alles ohne das Token herdr ---
hook "erlaubt: herdr agent list" 0 "herdr agent list"
hook "erlaubt: herdr agent get" 0 "herdr agent get orchestrator"
hook "erlaubt: herdr agent read mit --source" 0 "herdr agent read rust-developer-0043 --source visible"
hook "erlaubt: herdr agent wait mit Timeout" 0 "herdr agent wait rust-developer-0043 --timeout 1800000"
hook "erlaubt: say.sh" 0 "sh scripts/team/say.sh orchestrator 'fertig'"
hook "erlaubt: spawn.sh" 0 "sh .claude/skills/spawn-worker/spawn.sh rust-developer 0043 chore/0043-x"
hook "erlaubt: git status" 0 "git status --short"
hook "erlaubt: Dateiname mit herdr im Pfad (adr-0013-herdr-laufzeit)" 0 "git add docs/adr/adr-0013-herdr-laufzeit-und-transport.md"
hook "erlaubt: leerer Befehl ohne herdr" 0 ""

# --- verboten ---
hook "verboten: agent prompt" 2 "herdr agent prompt product-owner-0043 'Spec freigegeben'"
hook "verboten: env herdr" 2 "env herdr agent prompt x y"
hook "verboten: --session vor dem Unterbefehl" 2 "herdr --session talker agent prompt x y"
hook "verboten: absoluter Pfad" 2 "~/.local/bin/herdr agent prompt x y"
hook "verboten: sh -c" 2 "sh -c 'herdr agent prompt x y'"
hook "verboten: Variablenzuweisung davor" 2 "FOO=1 herdr agent list"
hook "verboten: pane close" 2 "herdr pane close p1"
hook "verboten: herdr allein" 2 "herdr"
hook "verboten: Lesebefehl mit ; und zweitem herdr-Befehl" 2 "herdr agent list; herdr pane close p1"
hook "verboten: Lesebefehl mit &&" 2 "herdr agent list && herdr agent prompt x y"
hook "verboten: Lesebefehl mit |" 2 "herdr agent list | cat"
hook "verboten: Lesebefehl mit Umleitung" 2 "herdr agent list > datei"
hook "verboten: Lesebefehl mit \$( )" 2 "herdr agent read \$(whoami)"
hook "verboten: Lesebefehl mit Backticks" 2 "herdr agent get \`whoami\`"
hook "verboten: zweite Zeile" 2 "herdr agent list
herdr pane close p1"
hook "verboten: herdr agent listing (kein Wortende)" 2 "herdr agent listing"
hook "verboten: herdr als Argument eines anderen Befehls" 2 "echo x; herdr agent prompt x y"

# --- kaputte Eingabe ---
printf '%s' 'kein json' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: Eingabe ist kein JSON (fail-closed)" 2 $?
printf '' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: leere Eingabe (fail-closed)" 2 $?

# --- fail-closed ohne jq ---
mkdir -p "$TMP/leer"
printf '%s' '{"tool_input":{"command":"git status"}}' >"$TMP/ok.json"
PATH="$TMP/leer" /bin/sh "$HOOK" <"$TMP/ok.json" >/dev/null 2>"$TMP/err"
assert_eq "verboten: jq fehlt im PATH (fail-closed)" 2 $?
assert_contains "jq fehlt: Begründung nennt jq" jq "$(cat "$TMP/err")"

th_summary
