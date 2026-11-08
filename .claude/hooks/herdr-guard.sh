#!/bin/sh
# PreToolUse-Hook (Matcher Bash, alle Rollen, ADR-0013): keine direkten herdr-Aufrufe außer
# `herdr agent list|get|read|wait` als ganzer Befehl. Jeder andere Befehl mit dem Token herdr
# (auch env herdr …, herdr --session …, absoluter Pfad, sh -c) → Exit 2 mit Begründung auf stderr.
# Pane-Steuerung und Nachrichten laufen über spawn.sh und say.sh; deren Befehlstext enthält das Token nicht.
# Eine Hürde gegen Versehen, keine Sicherheitsgrenze: verschleierte Aufrufe fängt sie nicht.
deny() { echo "herdr-guard: $1" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || deny "jq fehlt im PATH, Bash bleibt gesperrt"

# \A…\z statt ^…$: kein Treffer über mehrere Zeilen, kein angehängter Befehl.
cat | jq -e '(.tool_input.command // "") as $c
  | ($c | test("(^|[^A-Za-z0-9_-])herdr($|[^A-Za-z0-9_-])") | not)
    or ($c | test("\\Aherdr agent (list|get|read|wait)( [^\\n;&|`$()<>\\\\]*)?\\z"))' >/dev/null 2>&1 \
  || deny "herdr nur als: herdr agent list|get|read|wait (ohne ; & | \` \$ ( ) < >); Pane-Steuerung über spawn.sh, Nachrichten über say.sh"
