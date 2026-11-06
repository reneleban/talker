#!/bin/sh
# Startet eine Rollen-Session als Kachel im Ghostty-Fenster des Orchestrators (ADR-0010).
#   spawn.sh init <ticket> <feature-branch>
#   spawn.sh <rolle> <ticket> <feature-branch>
#   spawn.sh close <session-name>
set -eu

die() { echo "spawn: $*" >&2; exit 1; }

MAIN=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
WT_ROOT="$(dirname "$MAIN")/talker-wt"
INTEGRATION="$WT_ROOT/integration"

require_clean() {
  [ -z "$(git -C "$1" status --porcelain)" ] || die "$1 hat uncommittete Änderungen"
}

session_pid() {
  pgrep -f "^claude --agent [a-z-]+ --name $1( |\$)" | head -n 1
}

if [ "${1:-}" = close ]; then
  [ $# -eq 2 ] || die "Aufruf: spawn.sh close <session-name>"
  PID=$(session_pid "$2") || true
  [ -n "$PID" ] || die "keine laufende Session $2"
  # SIGTERM beendet Claude sauber; die Kachel schließt sich mit dem Prozess.
  kill -TERM "$PID"
  echo "$2 beendet"
  exit 0
fi

[ $# -eq 3 ] || die "Aufruf: spawn.sh init|<rolle> <ticket> <feature-branch> | spawn.sh close <session-name>"
ROLE=$1; TICKET=$2; FEATURE=$3
case $TICKET in [0-9][0-9][0-9][0-9]) ;; *) die "Ticket muss vierstellig sein: $TICKET" ;; esac

if [ "$ROLE" = init ]; then
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

[ -n "${CLAUDE_PID:-}" ] || die "CLAUDE_PID fehlt: spawn.sh muss aus der Orchestrator-Session laufen"
[ -d "$INTEGRATION" ] || die "Integrations-Worktree fehlt, zuerst: spawn.sh init $TICKET $FEATURE"

case $ROLE in
  product-owner|architect|rust-developer|test-engineer|release-engineer)
    DIR="$WT_ROOT/$ROLE"
    if [ -d "$DIR" ]; then
      require_clean "$DIR"
      git -C "$DIR" switch -C "wt/$ROLE" "$FEATURE"
    else
      git -C "$MAIN" worktree add -B "wt/$ROLE" "$DIR" "$FEATURE"
    fi ;;
  tech-lead|code-reviewer|security-auditor|performance-engineer|merger)
    DIR="$INTEGRATION" ;;
  *) die "unbekannte Rolle: $ROLE" ;;
esac

NAME="$ROLE-$TICKET"
[ -z "$(session_pid "$NAME")" ] || die "Session $NAME läuft bereits"
# Geerbte CLAUDE*-Variablen entfernen: sonst gilt die neue Session als Kind der
# startenden (kein eigener Posteingang, CLAUDE_EFFORT überschreibt das Frontmatter).
# Kein Start-Prompt: Den schickt Claude in einer Kachel ohne Fokus nicht ab.
# Der Auftrag kommt per SendMessage.
CMD="/bin/zsh -lc 'unset -m \"CLAUDE*\"; exec claude --agent $ROLE --name $NAME --permission-mode auto --add-dir \"$MAIN\"'"
# Erste Rolle rechts neben den Orchestrator, weitere unter die letzte
# Rollen-Kachel; danach gleichmäßig verteilen.
osascript - "$CLAUDE_PID" "$DIR" "$CMD" "$NAME" >/dev/null <<'OSA' || die "Ghostty-Kachel konnte nicht geöffnet werden"
on run argv
  set orchPid to (item 1 of argv) as integer
  tell application "Ghostty"
    set orch to missing value
    set orchTab to missing value
    repeat with w in windows
      repeat with t in tabs of w
        repeat with term in terminals of t
          if (pid of term) is orchPid then
            set orch to term
            set orchTab to t
          end if
        end repeat
      end repeat
    end repeat
    if orch is missing value then error "Orchestrator-Terminal nicht gefunden"
    set anchor to orch
    set dir to right
    repeat with term in terminals of orchTab
      if (id of term) is not (id of orch) then
        set anchor to term
        set dir to down
      end if
    end repeat
    set cfg to new surface configuration
    set initial working directory of cfg to item 2 of argv
    set command of cfg to item 3 of argv
    set wait after command of cfg to false
    set newTerm to split anchor direction dir with configuration cfg
    set name override of newTerm to item 4 of argv
    perform action "equalize_splits" on orch
  end tell
end run
OSA

# Warten, bis die Session ihren Posteingang geöffnet hat (max. 60 s).
i=0
while [ $i -lt 60 ]; do
  PID=$(session_pid "$NAME") || true
  if [ -n "$PID" ] && { [ -S "/tmp/cc-socks/$PID.sock" ] || [ -S "/tmp/cc-socks-$(id -u)/$PID.sock" ]; }; then
    echo "$NAME bereit in $DIR"
    exit 0
  fi
  sleep 1; i=$((i + 1))
done
die "$NAME nach 60 s nicht erreichbar"
