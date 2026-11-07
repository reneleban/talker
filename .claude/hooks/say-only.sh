#!/bin/sh
# PreToolUse-Hook (Matcher Bash) für Rollen, die nur melden dürfen (tech-lead, ADR-0013).
# Durch kommt ausschließlich: sh scripts/team/say.sh <ziel> '<text>' — ganzer Befehl, Text in einfachen
# Anführungszeichen ohne weiteres '. Alles andere (auch fehlendes jq) → Exit 2 mit Begründung auf stderr.
deny() { echo "say-only: $1" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || deny "jq fehlt im PATH, Bash bleibt gesperrt"

# \A…\z statt ^…$: kein Treffer über mehrere Zeilen, kein angehängter Befehl.
cat | jq -e '(.tool_input.command // "") | test("\\Ash scripts/team/say\\.sh [a-z][a-z0-9_-]{0,31} '"'"'[^'"'"']+'"'"'\\z")' >/dev/null 2>&1 \
  || deny "erlaubt ist nur: sh scripts/team/say.sh <ziel> '<text>' (Text ohne ')"
