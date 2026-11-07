#!/bin/sh
# Einstieg `make dev` (ADR-0013): herdr-Session starten, Orchestrator sicherstellen, verbinden.
# TEAM_SESSION (Default talker) ist die Test-Naht für die Skript-Tests.
set -eu

die() { echo "dev: $*" >&2; exit 1; }

for prog in herdr claude jq; do
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

shell_free() { # <pane>: Shell im Vordergrund, kein Programm davor
  h pane process-info --pane "$1" | jq -e '.result.process_info | .shell_pid as $s | all(.foreground_processes[]; .pid == $s)' >/dev/null
}

# Eine frische Pane ist kurz „busy", bis ihre Shell den Prompt zeigt: agent_pane_busy wiederholen (max. 15 s).
# agent_not_ready (z. B. Vertrauensdialog) ist kein Abbruch: Der Mensch beantwortet ihn nach dem Verbinden.
start_orchestrator() { # <pane>
  i=0
  while :; do
    err=$(h agent start orchestrator --kind claude --pane "$1" --timeout 60000 -- --name orchestrator 2>&1 >/dev/null) \
      && return 0
    case $err in *agent_pane_busy*) ;; *) break ;; esac
    i=$((i + 1))
    [ $i -le 75 ] || break
    sleep 0.2
  done
  echo "dev: orchestrator nicht bereit: $err" >&2
}

if ! h agent list | jq -e '.result.agents[] | select(.name == "orchestrator")' >/dev/null; then
  WS=$(h workspace list | jq -r '.result.workspaces | sort_by(.number) | .[0].workspace_id // empty')
  if [ -z "$WS" ]; then
    PANE=$(h workspace create --cwd "$MAIN" --label talker --no-focus | jq -r '.result.root_pane.pane_id')
    start_orchestrator "$PANE"
  else
    TAB=$(h tab list --workspace "$WS" | jq -r '.result.tabs | sort_by(.number) | .[0].tab_id')
    PANE=$(h pane list --workspace "$WS" | jq -r --arg t "$TAB" '[.result.panes[] | select(.tab_id == $t)] | .[0].pane_id')
    if h agent list | jq -e --arg p "$PANE" '.result.agents[] | select(.pane_id == $p and .agent == "claude" and (.name // "") == "")' >/dev/null; then
      # Nach einem Server-Neustart nimmt herdr Claude ohne Namen wieder auf (A5); benannte Agenten bleiben unberührt
      h agent rename "$PANE" orchestrator >/dev/null
    else
      shell_free "$PANE" || PANE=$(h pane split "$PANE" --direction right --cwd "$MAIN" --no-focus | jq -r '.result.pane.pane_id')
      start_orchestrator "$PANE"
    fi
  fi
fi

exec herdr session attach "$S"
