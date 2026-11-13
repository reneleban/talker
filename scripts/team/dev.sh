#!/bin/sh
# Einstieg `make dev` (ADR-0013): herdr-core-Server sicherstellen, Orchestrator starten, verbinden.
set -eu

die() { echo "dev: $*" >&2; exit 1; }

for prog in hc claude jq; do
  command -v "$prog" >/dev/null 2>&1 || die "$prog fehlt im PATH"
done

MAIN=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")

hc server status >/dev/null 2>&1 || hc server start >/dev/null 2>&1 || die "herdr-core-Server startet nicht"

if ! hc ls --json | jq -e '.terminals[] | select(.name == "orchestrator")' >/dev/null; then
  # Ohne CLAUDE*-Variablen starten, sonst gilt der Orchestrator als Kind der aufrufenden Claude-Session.
  # shellcheck disable=SC2046
  env $(env | sed -n 's/^\(CLAUDE[A-Za-z_]*\)=.*/-u \1/p') \
    hc run --bg --name orchestrator --cwd "$MAIN" --no-parent --keep claude --name orchestrator >/dev/null \
    || die "orchestrator startet nicht"
fi

exec hc attach orchestrator
