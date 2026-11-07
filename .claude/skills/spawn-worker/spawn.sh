#!/bin/sh
# Startet eine Rollen-Session als herdr-Pane (ADR-0013).
#   spawn.sh init <ticket> <feature-branch>
#   spawn.sh <rolle> <ticket> <feature-branch>
#   spawn.sh close <name>
# Exit: 0 bereit · 1 Abbruch vor jeder Änderung · 2 Aufruf · 3 nach einer Änderung nicht bereit/herdr-Fehler.
# SPAWN_READY_TIMEOUT_MS (Default 60000): Bereit-Frist (Test-Naht).
set -eu

die() { echo "spawn: $*" >&2; exit "${2:-1}"; }

MAIN=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
WT_ROOT="$(dirname "$MAIN")/talker-wt"
INTEGRATION="$WT_ROOT/integration"
READY_MS=${SPAWN_READY_TIMEOUT_MS:-60000}
ROLES_RE="product-owner|architect|tech-lead|rust-developer|test-engineer|code-reviewer|security-auditor|performance-engineer|merger|release-engineer"

require_clean() {
  [ -z "$(git -C "$1" status --porcelain)" ] || die "$1 hat uncommittete Änderungen"
}

USAGE="Aufruf: spawn.sh init <ticket> <feature-branch> | [--instance] <rolle> <ticket> <feature-branch> | close <name>"
need_herdr() {
  [ "${HERDR_ENV:-}" = 1 ] || die "außerhalb von herdr (HERDR_ENV ist nicht 1), Einstieg: make dev"
}

INSTANCE=0
while [ $# -gt 0 ]; do
  case $1 in
    --instance) INSTANCE=1; shift ;;
    -*) die "unbekannte Option: $1. $USAGE" 2 ;;
    *) break ;;
  esac
done

if [ "${1:-}" = close ]; then
  [ $# -eq 2 ] || die "$USAGE" 2
  need_herdr
  [ "$2" != orchestrator ] || die "orchestrator wird nicht über spawn.sh beendet"
  PANE=$(herdr agent get "$2" 2>/dev/null | jq -r '.result.agent.pane_id // empty') || true
  [ -n "$PANE" ] || die "keine laufende Session $2"
  herdr pane close "$PANE" >/dev/null
  echo "$2 beendet"
  exit 0
fi

[ $# -eq 3 ] || die "$USAGE" 2
ROLE=$1; TICKET=$2; FEATURE=$3

if [ "$ROLE" = init ]; then
  case $TICKET in [0-9][0-9][0-9][0-9]) ;; *) die "Ticket muss vierstellig sein: $TICKET" ;; esac
  git -C "$MAIN" rev-parse --verify --quiet "refs/heads/$FEATURE" >/dev/null \
    || git -C "$MAIN" branch "$FEATURE" main
  if [ -d "$INTEGRATION" ]; then
    require_clean "$INTEGRATION"
    git -C "$INTEGRATION" switch "$FEATURE"
  else
    mkdir -p "$WT_ROOT"
    git -C "$MAIN" worktree add "$INTEGRATION" "$FEATURE"
  fi
  echo "$INTEGRATION"
  exit 0
fi

# Prüfungen, alle vor jeder Änderung (kein Worktree, keine Pane)
need_herdr
case $TICKET in [0-9][0-9][0-9][0-9]) ;; *) die "Ticket muss vierstellig sein: $TICKET" ;; esac
case $ROLE in
  product-owner|architect|rust-developer|test-engineer|release-engineer) OWN=1; DIR="$WT_ROOT/$ROLE" ;;
  tech-lead|code-reviewer|security-auditor|performance-engineer|merger) OWN=0; DIR="$INTEGRATION" ;;
  *) die "unbekannte Rolle: $ROLE" ;;
esac
[ -d "$INTEGRATION" ] || die "Integrations-Worktree fehlt, zuerst: spawn.sh init $TICKET $FEATURE"

# Obergrenze: Umgebungsvariable (auch leer) vor team.env des Haupt-Checkouts; der Orchestrator zählt nicht.
if [ "${TEAM_MAX_SESSIONS+set}" = set ]; then
  MAX=$TEAM_MAX_SESSIONS
else
  MAX=$(sed -n 's/^TEAM_MAX_SESSIONS=//p' "$MAIN/team.env" 2>/dev/null | head -n 1)
fi
case $MAX in ''|0*|*[!0-9]*) die "TEAM_MAX_SESSIONS ungültig: '$MAX'" ;; esac
AGENTS=$(herdr agent list)
ACTIVE=$(printf '%s' "$AGENTS" | jq --arg re "^($ROLES_RE)-[0-9]{4}(-[0-9]+)?\$" '[.result.agents[] | select((.name // "") | test($re))] | length')
[ $((ACTIVE + 1)) -le "$MAX" ] || die "Obergrenze erreicht: $ACTIVE Rollen-Sessions aktiv, TEAM_MAX_SESSIONS=$MAX"

[ "$INSTANCE" = 0 ] || [ "$OWN" = 1 ] || die "--instance gibt es nur für Rollen mit eigenem Worktree, nicht für $ROLE"

# Name: Basisname, bei laufender Basis mit --instance die kleinste freie Instanz n >= 2 (Lücken werden gefüllt).
BASE="$ROLE-$TICKET"
BRANCH="wt/$ROLE"
NAME=$BASE
NAMES=$(printf '%s' "$AGENTS" | jq -r '[.result.agents[].name // empty] | join(" ")')
running() { case " $NAMES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
if running "$BASE"; then
  [ "$INSTANCE" = 1 ] || die "Session $BASE läuft bereits (weitere Instanz der Rolle: --instance)"
  n=2
  while running "$BASE-$n"; do n=$((n + 1)); done
  NAME="$BASE-$n"; DIR="$WT_ROOT/$ROLE-$n"; BRANCH="wt/$ROLE-$n"
fi
[ ${#NAME} -le 32 ] || die "Name länger als 32 Zeichen (herdr): $NAME"
[ "$OWN" = 0 ] || [ ! -d "$DIR" ] || require_clean "$DIR"

if [ "$OWN" = 1 ]; then
  if [ -d "$DIR" ]; then
    git -C "$DIR" switch -C "$BRANCH" "$FEATURE"
  else
    git -C "$MAIN" worktree add -B "$BRANCH" "$DIR" "$FEATURE"
  fi
fi

# Pane-ID immer aus der JSON-Antwort von herdr, nie geraten.
new_pane() {
  if [ "$ROLE" = product-owner ]; then
    # Neben den Orchestrator: seine Pane ist die des Aufrufers.
    herdr pane split "$HERDR_PANE_ID" --direction right --cwd "$DIR" --no-focus | jq -r '.result.pane.pane_id'
  else
    # Tabs <ticket>, <ticket>-2, …: der erste mit Platz (max. 4 Panes), sonst ein neuer.
    TABS=$(herdr tab list --workspace "$HERDR_WORKSPACE_ID")
    TAB=$(printf '%s' "$TABS" | jq -r --arg l "$TICKET" \
      '[.result.tabs[] | select((.label | test("^" + $l + "(-[0-9]+)?$")) and .pane_count < 4)] | sort_by(.number) | .[0].tab_id // empty')
    if [ -z "$TAB" ]; then
      LABEL=$(printf '%s' "$TABS" | jq -r --arg l "$TICKET" \
        '[.result.tabs[].label] as $ls | [range(1; 100)] | map(if . == 1 then $l else "\($l)-\(.)" end) | map(select(. as $c | $ls | index($c) | not)) | .[0]')
      herdr tab create --workspace "$HERDR_WORKSPACE_ID" --label "$LABEL" --cwd "$DIR" --no-focus | jq -r '.result.root_pane.pane_id'
    else
      # 2. Pane rechts von 1, 3. unter 1, 4. unter 2 (Reihenfolge = Anlegereihenfolge, nicht Layout-Reihenfolge; Pane-IDs zählen hexadezimal: p9, pA, …, pF, p10)
      TPANES=$(herdr pane list --workspace "$HERDR_WORKSPACE_ID" | jq -c --arg t "$TAB" '[.result.panes[] | select(.tab_id == $t) | .pane_id] | sort_by(split(":")[1][1:] | [length, .])')
      case $(printf '%s' "$TPANES" | jq 'length') in
        1) AT=$(printf '%s' "$TPANES" | jq -r '.[0]'); DIRN=right ;;
        2) AT=$(printf '%s' "$TPANES" | jq -r '.[0]'); DIRN=down ;;
        *) AT=$(printf '%s' "$TPANES" | jq -r '.[1]'); DIRN=down ;;
      esac
      herdr pane split "$AT" --direction "$DIRN" --cwd "$DIR" --no-focus | jq -r '.result.pane.pane_id'
    fi
  fi
}

PANE=$(new_pane) || die "Pane konnte nicht angelegt werden (Worktree $DIR besteht bereits)" 3
[ -n "$PANE" ] || die "keine Pane angelegt (Worktree $DIR besteht bereits)" 3

# Eine frische Pane ist kurz „busy", bis ihre Shell den Prompt zeigt: agent_pane_busy wiederholen.
START=$(date +%s)
i=0
while :; do
  err=$(herdr agent start "$NAME" --kind claude --pane "$PANE" --timeout "$READY_MS" -- \
    --agent "$ROLE" --name "$NAME" --permission-mode auto --add-dir "$MAIN" 2>&1 >/dev/null) && break
  case $err in
    *agent_pane_busy*) i=$((i + 1)); [ $i -le 75 ] || die "Pane $PANE wird nicht frei (Worktree $DIR und Pane bestehen)" 3; sleep 0.2 ;;
    *agent_not_ready*)
      REST=$((READY_MS - ($(date +%s) - START) * 1000))
      [ $REST -gt 0 ] || REST=1
      herdr agent wait "$NAME" --until idle --until done --timeout "$REST" >/dev/null 2>&1 \
        || die "$NAME nach $((READY_MS / 1000)) s nicht bereit (Pane $PANE bleibt offen, Worktree $DIR besteht)" 3
      break ;;
    *) die "herdr agent start: $err (Pane $PANE und Worktree $DIR bestehen)" 3 ;;
  esac
done

echo "$NAME bereit in $DIR"
