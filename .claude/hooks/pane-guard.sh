#!/bin/sh
# PreToolUse-Hook (Matcher Bash, alle Rollen, ADR-0013): Positivliste gegen direkte herdr-Aufrufe.
# Das Token herdr (ohne Rücksicht auf Groß-/Kleinschreibung, APFS) ist im Befehlstext überall gesperrt, außer
#  1. `herdr agent list|get|read|wait` als ganzer Befehl (Orchestrator), und
#  2. einem einzelnen einfachen Befehl ohne ; & | Backtick $( <( >( =( (e: (+ und Zeilenumbruch, der beginnt mit
#     git commit, git log, grep, rg, gh pr create, gh pr merge, sh scripts/team/say.sh oder
#     sh .claude/skills/spawn-worker/spawn.sh (spawn.sh prüft den Branch selbst).
# Pane-Steuerung und Nachrichten laufen über spawn.sh und say.sh.
# Eine Hürde gegen Versehen, keine Sicherheitsgrenze: verschleierte Aufrufe (Token zerlegt, rg --pre) fängt sie nicht.
deny() { echo "pane-guard: $1" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || deny "jq fehlt im PATH, Bash bleibt gesperrt"

# \A…\z statt ^…$: kein Treffer über mehrere Zeilen, kein angehängter Befehl.
cat | jq -e '(.tool_input.command // "") as $c
  | ($c | test("herdr"; "i") | not)
    or ($c | test("\\Aherdr agent (list|get|read|wait)( [^\\n;&|`$()<>\\\\]*)?\\z"))
    or (($c | test("\\A(git commit|git log|grep|rg|gh pr create|gh pr merge|sh scripts/team/say\\.sh|sh \\.claude/skills/spawn-worker/spawn\\.sh)( |\\z)"))
        and ($c | test("[;&|`\\n]|\\$\\(|[<>=]\\(|\\(e:|\\(\\+") | not))' >/dev/null 2>&1 \
  || deny "herdr nur als: herdr agent list|get|read|wait, oder Text in einem einfachen git commit|git log|grep|rg|gh pr create|gh pr merge|sh scripts/team/say.sh|sh .claude/skills/spawn-worker/spawn.sh (ohne ; & | \` \$( und Zeilenumbruch; lange Texte per git commit -F <datei> oder gh pr create --body-file <datei>); Pane-Steuerung über spawn.sh, Nachrichten über say.sh"
