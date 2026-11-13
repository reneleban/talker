#!/bin/sh
# Startet eine Rollen-Session als herdr-core-Terminal (ADR-0013).
#   spawn.sh init <ticket> <feature-branch>
#   spawn.sh [--kind claude|codex] [--instance] <rolle> <ticket> <feature-branch>
#   spawn.sh close <name>
# Exit: 0 bereit · 1 Abbruch vor jeder Änderung · 2 Aufruf · 3 nach einer Änderung nicht bereit/hc-Fehler.
# SPAWN_READY_TIMEOUT_S (Default 60): Bereit-Frist (Test-Naht).
set -eu

die() { echo "spawn: $1" >&2; exit "${2:-1}"; }

MAIN=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
WT_ROOT="$(dirname "$MAIN")/talker-wt"
INTEGRATION="$WT_ROOT/integration"
READY_S=${SPAWN_READY_TIMEOUT_S:-60}
# Einzige Rollenliste: eigener Worktree bzw. Integrations-Worktree; ROLES_RE und die Prüfung per in_roles leiten sich daraus ab.
OWN_ROLES="product-owner architect rust-developer test-engineer release-engineer"
SHARED_ROLES="tech-lead code-reviewer security-auditor performance-engineer merger"
ROLES_RE=$(printf '%s %s' "$OWN_ROLES" "$SHARED_ROLES" | tr ' ' '|')
in_roles() { case " $2 " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

check_feature() {
  printf '%s' "$FEATURE" | grep -Eq '^(feat|fix|chore)/[a-z0-9-]+$' \
    || die "Feature-Branch muss ^(feat|fix|chore)/[a-z0-9-]+\$ entsprechen: $FEATURE"
}

require_clean() {
  [ -z "$(git -C "$1" status --porcelain)" ] || die "$1 hat uncommittete Änderungen"
}

USAGE="Aufruf: spawn.sh init <ticket> <feature-branch> | [--kind claude|codex] [--instance] <rolle> <ticket> <feature-branch> | close <name>"
need_hc() {
  [ -n "${HC_TERMINAL_ID:-}" ] || die "außerhalb von herdr-core (HC_TERMINAL_ID fehlt), Einstieg: make dev"
}

INSTANCE=0
KIND=claude
while [ $# -gt 0 ]; do
  case $1 in
    --instance) INSTANCE=1; shift ;;
    --kind)
      [ $# -ge 2 ] || die "--kind braucht einen Wert (claude|codex). $USAGE" 2
      KIND=$2; shift 2 ;;
    -*) die "unbekannte Option: $1. $USAGE" 2 ;;
    *) break ;;
  esac
done

case $KIND in claude|codex) ;; *) die "unbekannter --kind: $KIND (erlaubt: claude|codex)" 2 ;; esac

if [ "${1:-}" = close ]; then
  [ $# -eq 2 ] || die "$USAGE" 2
  need_hc
  [ "$2" != orchestrator ] || die "orchestrator wird nicht über spawn.sh beendet"
  hc ls --json | jq -e --arg n "$2" '.terminals[] | select(.name == $n)' >/dev/null \
    || die "keine laufende Session $2"
  hc kill "$2" >/dev/null
  echo "$2 beendet"
  exit 0
fi

[ $# -eq 3 ] || die "$USAGE" 2
ROLE=$1; TICKET=$2; FEATURE=$3

if [ "$ROLE" = init ]; then
  case $TICKET in [0-9][0-9][0-9][0-9]) ;; *) die "Ticket muss vierstellig sein: $TICKET" ;; esac
  check_feature
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

# Prüfungen, alle vor jeder Änderung (kein Worktree, kein Terminal)
need_hc
case $TICKET in [0-9][0-9][0-9][0-9]) ;; *) die "Ticket muss vierstellig sein: $TICKET" ;; esac
check_feature
if in_roles "$ROLE" "$OWN_ROLES"; then OWN=1; DIR="$WT_ROOT/$ROLE"
elif in_roles "$ROLE" "$SHARED_ROLES"; then OWN=0; DIR="$INTEGRATION"
else die "unbekannte Rolle: $ROLE"
fi
[ -d "$INTEGRATION" ] || die "Integrations-Worktree fehlt, zuerst: spawn.sh init $TICKET $FEATURE"

# Obergrenze: Umgebungsvariable (auch leer) vor team.env des Haupt-Checkouts; der Orchestrator zählt nicht.
if [ "${TEAM_MAX_SESSIONS+set}" = set ]; then
  MAX=$TEAM_MAX_SESSIONS
else
  MAX=$(sed -n 's/^TEAM_MAX_SESSIONS=//p' "$MAIN/team.env" 2>/dev/null | head -n 1)
fi
case $MAX in ''|0*|*[!0-9]*) die "TEAM_MAX_SESSIONS ungültig: '$MAX'" ;; esac
TERMS=$(hc ls --json)
ACTIVE=$(printf '%s' "$TERMS" | jq --arg re "^($ROLES_RE)-[0-9]{4}(-[0-9]+)?\$" '[.terminals[] | select((.name // "") | test($re))] | length')
[ $((ACTIVE + 1)) -le "$MAX" ] || die "Obergrenze erreicht: $ACTIVE Rollen-Sessions aktiv, TEAM_MAX_SESSIONS=$MAX"

[ "$INSTANCE" = 0 ] || [ "$OWN" = 1 ] || die "--instance gibt es nur für Rollen mit eigenem Worktree, nicht für $ROLE"

# Name: Basisname, bei laufender Basis mit --instance die kleinste freie Instanz n >= 2 (Lücken werden gefüllt).
BASE="$ROLE-$TICKET"
BRANCH="wt/$ROLE"
NAME=$BASE
NAMES=$(printf '%s' "$TERMS" | jq -r '[.terminals[].name // empty] | join(" ")')
running() { case " $NAMES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
if running "$BASE"; then
  [ "$INSTANCE" = 1 ] || die "Session $BASE läuft bereits (weitere Instanz der Rolle: --instance)"
  n=2
  while running "$BASE-$n"; do n=$((n + 1)); done
  NAME="$BASE-$n"; DIR="$WT_ROOT/$ROLE-$n"; BRANCH="wt/$ROLE-$n"
fi
[ ${#NAME} -le 32 ] || die "Name länger als 32 Zeichen: $NAME"
[ "$OWN" = 0 ] || [ ! -d "$DIR" ] || require_clean "$DIR"

if [ "$OWN" = 1 ]; then
  if [ -d "$DIR" ]; then
    git -C "$DIR" switch -C "$BRANCH" "$FEATURE"
  else
    git -C "$MAIN" worktree add -B "$BRANCH" "$DIR" "$FEATURE"
  fi
fi

# Claude bekommt Rolle, Name, Modus und Haupt-Checkout; Codex startet ohne Argumente (Codex-Setup ist ein Folgeticket).
if [ "$KIND" = claude ]; then
  set -- claude --agent "$ROLE" --name "$NAME" --permission-mode auto --add-dir "$MAIN"
else
  set -- codex
fi

# Kein Parent-Bezug: Die Rolle überlebt den Orchestrator und tippt keine Notizen in ihn (Meldung nur per say.sh).
hc run --bg --name "$NAME" --cwd "$DIR" --label "ticket=$TICKET" --label "role=$ROLE" \
  --no-parent --no-notify "$@" >/dev/null \
  || die "Terminal $NAME konnte nicht gestartet werden (Worktree $DIR besteht bereits)" 3

# Bereit = der Agent hat sein Warten beendet und ist nicht blockiert (z. B. Ordner-Vertrauensdialog).
hc wait "$NAME" --timeout "$READY_S" >/dev/null 2>&1 || true
STATUS=$(hc get "$NAME" --json | jq -r '.status.state // "none"')
case $STATUS in
  blocked | none) die "$NAME nach ${READY_S} s nicht bereit (Status $STATUS; Terminal bleibt offen, Worktree $DIR besteht)" 3 ;;
esac

echo "$NAME bereit in $DIR"
