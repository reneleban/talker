#!/bin/sh
# Einstieg `make dev` (ADR-0013): herdr-Session starten, Orchestrator sicherstellen, verbinden.
# TEAM_SESSION (Default talker) ist die Test-Naht für die Skript-Tests.
set -eu

die() { echo "dev: $*" >&2; exit 1; }

for prog in herdr claude; do
  command -v "$prog" >/dev/null 2>&1 || die "$prog fehlt im PATH"
done

S=${TEAM_SESSION:-talker}
MAIN=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
h() { herdr --session "$S" "$@"; }

if ! herdr session list --json | jq -e --arg s "$S" '.sessions[] | select(.name == $s and .running)' >/dev/null; then
  # Panes erben die Umgebung des Servers, nicht die des Orchestrators (B9): ohne CLAUDE*-Variablen
  # starten, sonst gilt jede Rolle als Kind dieser Session.
  # shellcheck disable=SC2046
  env $(env | sed -n 's/^\(CLAUDE[A-Za-z_]*\)=.*/-u \1/p') \
    nohup herdr --session "$S" server >/dev/null 2>&1 &
  i=0
  until h workspace list >/dev/null 2>&1; do
    i=$((i + 1))
    [ $i -le 100 ] || die "herdr-Server '$S' startet nicht innerhalb von 10 s"
    sleep 0.1
  done
fi

ROOT=$(h workspace create --cwd "$MAIN" --label talker --no-focus | jq -r '.result.root_pane.pane_id')
# agent_not_ready (z. B. Ordner-Vertrauensdialog) ist kein Abbruch: Der Mensch beantwortet ihn nach dem Verbinden.
h agent start orchestrator --kind claude --pane "$ROOT" --timeout 60000 -- --name orchestrator >/dev/null \
  || echo "dev: orchestrator nicht bereit (siehe herdr-Status)" >&2

exec herdr session attach "$S"
