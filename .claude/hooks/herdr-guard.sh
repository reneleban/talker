#!/bin/sh
# PreToolUse-Hook (Matcher Bash, alle Rollen, ADR-0013): keine direkten herdr-Aufrufe außer
# `herdr agent list|get|read|wait` als ganzer Befehl. Gesperrt ist das Token herdr (ohne Rücksicht auf
# Groß-/Kleinschreibung, APFS) in Befehlsposition: am Anfang, nach ; & | ( ` oder Zeilenumbruch, nach
# env/exec/command/nohup/time/sudo und Variablenzuweisungen, nach sh|bash|zsh -c mit Anführungszeichen,
# auch mit Pfad davor. Als Argument (grep, git commit -m, say.sh, gh pr --title) bleibt es frei.
# Pane-Steuerung und Nachrichten laufen über spawn.sh und say.sh.
# Eine Hürde gegen Versehen, keine Sicherheitsgrenze: verschleierte Aufrufe fängt sie nicht.
deny() { echo "herdr-guard: $1" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || deny "jq fehlt im PATH, Bash bleibt gesperrt"

# \x27 = einfaches Anführungszeichen (im Shell-Einfachzitat nicht tippbar). \A…\z statt ^…$: kein Treffer über mehrere Zeilen, kein angehängter Befehl.
cat | jq -e '(.tool_input.command // "") as $c
  | ($c | test("(\\A|[;&|(`\\n]|\\b(sh|bash|zsh) -c)\\s*[\"\\x27]?\\s*((env|exec|command|nohup|time|sudo|[A-Za-z_][A-Za-z0-9_]*=[^\\s]*)\\s+)*([^\\s;&|()`\"\\x27]*/)?herdr($|[^A-Za-z0-9_-])"; "i") | not)
    or ($c | test("\\Aherdr agent (list|get|read|wait)( [^\\n;&|`$()<>\\\\]*)?\\z"))' >/dev/null 2>&1 \
  || deny "herdr nur als: herdr agent list|get|read|wait (ohne ; & | \` \$ ( ) < >); Pane-Steuerung über spawn.sh, Nachrichten über say.sh"
