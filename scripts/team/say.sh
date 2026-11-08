#!/bin/sh
# Nachricht an eine andere Rolle senden (ADR-0013). Agentenneutral: nur herdr + jq.
#   sh scripts/team/say.sh <ziel> '<text>'
# Exit 0 = zugestellt (nicht: verarbeitet) · 1 = Umgebung/Absender/Ziel/Zustellung · 2 = Aufruf.
# SAY_RETRY_TIMEOUT_MS (Default 20000): Wartezeit zwischen Versuchen bei blockiertem Ziel (Test-Naht).
set -eu

die() { echo "say: $1" >&2; exit "${2:-1}"; }

[ $# -eq 2 ] || die "Aufruf: say.sh <ziel> '<text>'" 2
TARGET=$1; TEXT=$2
printf '%s' "$TARGET" | grep -Eq '^[a-z][a-z0-9_-]{0,31}$' || die "ungültiges Ziel: $TARGET" 2
[ -n "$TEXT" ] || die "Text ist leer" 2

[ "${HERDR_ENV:-}" = 1 ] && [ -n "${HERDR_PANE_ID:-}" ] || die "außerhalb von herdr (HERDR_ENV/HERDR_PANE_ID fehlen)"

AGENTS=$(herdr agent list)
# Absender = Agent der eigenen Pane, nie ein Parameter: so gibt sich keine Rolle als andere aus.
SENDER=$(printf '%s' "$AGENTS" | jq -r --arg p "$HERDR_PANE_ID" '[.result.agents[] | select(.pane_id == $p) | .name // empty] | .[0] // empty')
[ -n "$SENDER" ] || die "kein benannter Agent in der eigenen Pane $HERDR_PANE_ID, Absender unbekannt"
printf '%s' "$AGENTS" | jq -e --arg t "$TARGET" '.result.agents[] | select(.name == $t)' >/dev/null \
  || die "Ziel nicht gefunden: $TARGET"

# Ohne --wait: sonst hinge der Absender am Turn des Empfängers.
attempt=1
while :; do
  err=$(herdr agent prompt "$TARGET" "[talker von $SENDER] $TEXT" 2>&1 >/dev/null) && exit 0
  case $err in *agent_blocked*) ;; *) die "herdr: $err" ;; esac
  [ $attempt -lt 3 ] || die "$TARGET blockiert nach 3 Versuchen"
  herdr agent wait "$TARGET" --until idle --until done --timeout "${SAY_RETRY_TIMEOUT_MS:-20000}" >/dev/null 2>&1 || true
  attempt=$((attempt + 1))
done
