#!/bin/sh
# Nachricht an eine andere Rolle senden (ADR-0013). Agentenneutral: nur hc + jq.
#   sh scripts/team/say.sh <ziel> '<text>'
# Exit 0 = zugestellt (nicht: verarbeitet) · 1 = Umgebung/Absender/Ziel/Zustellung · 2 = Aufruf.
# SAY_RETRY_TIMEOUT_S (Default 20): Wartezeit zwischen Versuchen bei blockiertem Ziel (Test-Naht).
set -eu

die() { echo "say: $1" >&2; exit "${2:-1}"; }

[ $# -eq 2 ] || die "Aufruf: say.sh <ziel> '<text>'" 2
TARGET=$1; TEXT=$2
printf '%s' "$TARGET" | grep -Eq '^[a-z][a-z0-9_-]{0,31}$' || die "ungültiges Ziel: $TARGET" 2
[ -n "$TEXT" ] || die "Text ist leer" 2

[ -n "${HC_TERMINAL_ID:-}" ] || die "außerhalb von herdr-core (HC_TERMINAL_ID fehlt)"

# Absender = Name des eigenen Terminals, nie ein Parameter. Nur ein Hinweis für den Empfänger, keine Authentifizierung.
SENDER=$(hc get "$HC_TERMINAL_ID" --json | jq -r '.name // empty')
[ -n "$SENDER" ] || die "eigenes Terminal $HC_TERMINAL_ID hat keinen Namen, Absender unbekannt"
hc ls --json | jq -e --arg t "$TARGET" '.terminals[] | select(.name == $t)' >/dev/null \
  || die "Ziel nicht gefunden: $TARGET"

attempt=1
while :; do
  err=$(hc send "$TARGET" "[talker von $SENDER] $TEXT" 2>&1 >/dev/null) && exit 0
  [ $attempt -lt 3 ] || die "$TARGET nicht erreichbar nach 3 Versuchen: $err"
  hc wait "$TARGET" --timeout "${SAY_RETRY_TIMEOUT_S:-20}" >/dev/null 2>&1 || true
  attempt=$((attempt + 1))
done
