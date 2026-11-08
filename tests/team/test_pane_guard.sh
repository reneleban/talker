#!/bin/sh
# S2: Bash-Wächter gegen direkte herdr-Aufrufe (alle Rollen). Reiner stdin→Exit-Code-Test, ohne herdr.
. "$(dirname "$0")/lib.sh"

HOOK="$TH_SRC/.claude/hooks/pane-guard.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/pane-guard.XXXXXX")
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

# --- N1: Groß-/Kleinschreibung (APFS) ---
hook "verboten: HERDR agent prompt" 2 "HERDR agent prompt x y"
hook "verboten: Herdr --session" 2 "Herdr --session talker agent send-keys x y"
hook "verboten: HERDR agent list | cat" 2 "HERDR agent list | cat"

# --- R1: Token nur in Befehlsposition, Argumente frei ---
hook "erlaubt: grep mit herdr im Text" 0 "grep -rn herdr docs"
hook "erlaubt: git log --grep herdr" 0 "git log --grep herdr"
hook "erlaubt: git commit -m mit herdr agent prompt" 0 "git commit -m 'fix: herdr agent prompt gesperrt'"
hook "erlaubt: git commit -m, Text beginnt mit herdr" 0 "git commit -m \"herdr agent prompt\""
hook "erlaubt: say.sh mit herdr-Lesebefehl im Text" 0 "sh scripts/team/say.sh orchestrator 'lies mit herdr agent read x'"
hook "erlaubt: gh pr create --title mit herdr" 0 "gh pr create --draft --title 'chore: herdr guard'"
hook "erlaubt: gh pr merge --subject mit herdr" 0 "gh pr merge --squash --subject 'fix: herdr agent prompt'"
hook "verboten: cat Pfad mit /herdr (nicht auf der Positivliste)" 2 "cat docs/herdr"
hook "verboten: git add Dateiname mit herdr (nicht auf der Positivliste)" 2 "git add docs/adr/adr-0013-herdr-laufzeit-und-transport.md"
hook "verboten: nach ; ohne Leerzeichen" 2 "git status;herdr pane close p1"
hook "verboten: nach &&" 2 "git status && herdr pane close p1"
hook "verboten: in Subshell" 2 "(herdr pane close p1)"
hook "verboten: in \$( )" 2 "echo \$(herdr pane close p1)"
hook "verboten: in Backticks" 2 "echo \`herdr pane close p1\`"
hook "verboten: bash -c mit Doppelanführungszeichen" 2 "bash -c \"herdr pane close p1\""
hook "verboten: env mit Variablen" 2 "env A=1 herdr pane close p1"
hook "verboten: Pfad am Befehlsanfang" 2 "/opt/homebrew/bin/herdr pane close p1"
hook "verboten: Pfad nach ;" 2 "x; ~/.local/bin/herdr pane close p1"

# --- N5/R6: Positivliste, Token überall gesperrt außer Lesebefehl und einfachem Befehl der Liste ---
while IFS= read -r c; do
  [ -n "$c" ] || continue
  hook "N5 verboten: $c" 2 "$c"
done <<'EOF'
eval herdr pane close p1
eval 'herdr pane close p1'
echo herdr | xargs -n1 true
xargs herdr pane close
xargs -n1 herdr pane close
find . -exec herdr pane close p1 ;
{ herdr pane close p1; }
if true; then herdr pane close p1; fi
! herdr pane close p1
2>/dev/null herdr pane close p1
\herdr pane close p1
X=herdr; $X pane close p1
timeout 5 herdr pane close p1
nice herdr pane close p1
sh -xc 'herdr pane close p1'
bash -lc 'herdr pane close p1'
dash -c 'herdr pane close p1'
sh  -c 'herdr pane close p1'
env -i herdr pane close p1
sudo -u x herdr pane close p1
osascript -e 'do shell script "herdr pane close p1"'
for i in 1; do herdr pane close p1; done
while true; do herdr pane close p1; done
grep x <(herdr pane close p1)
git log --format=%H > >(herdr pane close p1)
git commit -m x; herdr pane close p1
grep x && herdr pane close p1
gh pr create --title x | herdr pane close p1
grep $(herdr pane close p1)
grep `herdr pane close p1`
sh scripts/team/say.sh orchestrator x & herdr pane close p1
gh pr merge --subject 'a; herdr pane close p1'
echo git commit; herdr pane close p1
grepx herdr
EOF
hook "N5 verboten: Block über Zeilenumbrüche (if/then)" 2 "if true
then herdr pane close p1
fi"
hook "N5 verboten: rg, dann Zeilenumbruch und Token" 2 "rg x
herdr pane close p1"
hook "N5 verboten: git commit -m, Zeilenumbruch im Text vor dem Token" 2 "git commit -m 'a
herdr pane close p1'"
hook "N5 verboten: Lesebefehl, Zeilenumbruch, weiterer Befehl" 2 "herdr agent list
git status"
hook "N5 erlaubt: say.sh, ( vor dem Token im Fließtext" 0 "sh scripts/team/say.sh orchestrator 'fertig (herdr agent read x) und weiter'"
hook "N5 erlaubt: git commit -m mit Klammer vor dem Token" 0 "git commit -m 'fix(guard): (herdr) Positivliste'"
hook "N5 erlaubt: rg mit herdr" 0 "rg -n herdr docs"
hook "N5 erlaubt: git log --grep=herdr" 0 "git log --oneline --grep=herdr"

# --- R9: spawn.sh frei, auch mit dem Token im Branch-Namen (Feature-Branch dieses Tickets) ---
hook "R9 erlaubt: spawn.sh init mit Token im Branch" 0 "sh .claude/skills/spawn-worker/spawn.sh init 0043 chore/0043-parallele-sessions-herdr"
hook "R9 erlaubt: spawn.sh Rolle mit Token im Branch" 0 "sh .claude/skills/spawn-worker/spawn.sh release-engineer 0043 chore/0043-parallele-sessions-herdr"
hook "R9 verboten: spawn.sh, danach zweiter Befehl mit Token" 2 "sh .claude/skills/spawn-worker/spawn.sh init 0043 chore/0043-herdr; herdr pane close p1"
hook "R9 verboten: anderes Skript mit Token im Branch" 2 "sh .claude/skills/spawn-worker/other.sh init 0043 chore/0043-parallele-sessions-herdr"

# --- Q1: zsh =( ), (e: ) und (+ ) sind Ausführung im Argument ---
while IFS= read -r c; do
  [ -n "$c" ] || continue
  hook "Q1 verboten: $c" 2 "$c"
done <<'EOF'
grep x =(herdr agent prompt po ja)
rg x =(herdr pane list)
grep x pro*(e:herdr pane close p1:)
grep x pro*(+herdr_fn)
EOF

# --- kaputte Eingabe ---
printf '%s' 'kein json' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: Eingabe ist kein JSON (fail-closed)" 2 $?
printf '' | sh "$HOOK" >/dev/null 2>&1; assert_eq "verboten: leere Eingabe (fail-closed)" 2 $?

# --- N3: fehlende Hook-Datei oder leeres CLAUDE_PROJECT_DIR → Exit 2 (nicht 127, sonst fail-open) ---
CMD_SETTINGS=$(jq -r '.hooks.PreToolUse[0].hooks[0].command' "$TH_SRC/.claude/settings.json")
CMD_TECHLEAD=$(sed -n 's/^ *command: //p' "$TH_SRC/.claude/agents/tech-lead.md")
mkdir -p "$TMP/leer"
printf '%s' '{"tool_input":{"command":"git status"}}' >"$TMP/ok.json"
CLAUDE_PROJECT_DIR="$TH_SRC" sh -c "$CMD_SETTINGS" <"$TMP/ok.json" >/dev/null 2>&1
assert_eq "N3 settings: Hook mit gültigem Pfad lässt git status durch" 0 $?
for pair in "settings:$CMD_SETTINGS" "tech-lead:$CMD_TECHLEAD"; do
  n=${pair%%:*}; c=${pair#*:}
  CLAUDE_PROJECT_DIR="" sh -c "$c" <"$TMP/ok.json" >/dev/null 2>&1
  assert_eq "N3 $n: leeres CLAUDE_PROJECT_DIR → Exit 2" 2 $?
  CLAUDE_PROJECT_DIR="$TMP/leer" sh -c "$c" <"$TMP/ok.json" >/dev/null 2>&1
  assert_eq "N3 $n: fehlende Hook-Datei → Exit 2" 2 $?
done

# --- fail-closed ohne jq ---
PATH="$TMP/leer" /bin/sh "$HOOK" <"$TMP/ok.json" >/dev/null 2>"$TMP/err"
assert_eq "verboten: jq fehlt im PATH (fail-closed)" 2 $?
assert_contains "jq fehlt: Begründung nennt jq" jq "$(cat "$TMP/err")"

th_summary
