#!/bin/sh
# Prüft Berechtigungen, Agent-Definitionen und Doku per jq/grep (Ticket 0043, ohne herdr).
. "$(dirname "$0")/lib.sh"
cd "$TH_SRC" || exit 1

SETTINGS=.claude/settings.json

# --- T12: settings.json (AK-15) ---
if jq -e . "$SETTINGS" >/dev/null 2>&1; then ok "AK-15 settings.json ist gültiges JSON"; else fail "AK-15 settings.json ist gültiges JSON"; fi
in_list() { jq -e --arg e "$2" ".permissions.$1 | index(\$e) != null" "$SETTINGS" >/dev/null 2>&1; }
for e in 'Bash(sh scripts/team/say.sh *)' 'Bash(herdr agent list)' 'Bash(herdr agent get *)' 'Bash(herdr agent read *)' 'Bash(herdr agent wait *)'; do
  if in_list allow "$e"; then ok "AK-15 allow: $e"; else fail "AK-15 allow: $e"; fi
done
for e in 'Bash(herdr pane *)' 'Bash(herdr tab *)' 'Bash(herdr workspace *)' 'Bash(herdr session *)' 'Bash(herdr server *)' \
  'Bash(herdr agent start*)' 'Bash(herdr agent prompt*)' 'Bash(herdr agent send-keys*)' 'Bash(herdr agent rename*)' 'Bash(herdr agent attach*)'; do
  if in_list deny "$e"; then ok "AK-15 deny: $e"; else fail "AK-15 deny: $e"; fi
done
if git cat-file -e 586bff4:"$SETTINGS" 2>/dev/null; then
  for list in allow deny; do
    git show 586bff4:"$SETTINGS" | jq -r ".permissions.$list[]" | while IFS= read -r e; do
      if in_list "$list" "$e"; then echo "ok   bestehender $list-Eintrag: $e"; else echo "FAIL bestehender $list-Eintrag fehlt: $e"; fi
    done
  done >"${TMPDIR:-/tmp}/check_docs.$$"
  grep -q '^FAIL' "${TMPDIR:-/tmp}/check_docs.$$" && { grep '^FAIL' "${TMPDIR:-/tmp}/check_docs.$$"; fail "AK-15 bestehende Einträge unverändert"; } || ok "AK-15 bestehende Einträge unverändert"
  rm -f "${TMPDIR:-/tmp}/check_docs.$$"
else
  skip "bestehende Einträge: Basis-Commit 586bff4 nicht vorhanden"
fi


# --- Hilfen für die Agent-Definitionen ---
AGENTS_DIR=.claude/agents
frontmatter() { awk '/^---$/ { n++; next } n == 1' "$1"; }
fm_line() { frontmatter "$1" | grep "^$2:" || true; }
# has <name> <datei> <regex>: Datei enthält das Muster (Grundlage der grep-Prüfungen)
has() { if grep -Eq -e "$3" "$2"; then ok "$1"; else fail "$1 ($2: '$3' fehlt)"; fi; }
lacks() { if grep -Eq -e "$3" "$2"; then fail "$1 ($2: '$3' kommt vor)"; else ok "$1"; fi; }
FORBIDDEN='SendMessage|ListAgents|osascript|/tmp/cc-socks'

# Gemeinsam für jede Rolle (AK-16, AK-22): kein altes Messaging, Meldung über say.sh
agent_common() { # <rolle> [muster für die Meldung, Default: an den Orchestrator]
  f=$AGENTS_DIR/$1.md
  lacks "AK-16 $1: kein SendMessage/ListAgents/osascript/cc-socks" "$f" "$FORBIDDEN"
  lacks "AK-17 $1: keine Ghostty-Kacheln" "$f" 'Ghostty|Kachel'
  has "AK-16 $1: Meldung über say.sh" "$f" "${2:-scripts/team/say[.]sh orchestrator}"
}

# --- T13: tech-lead.md (AK-15, AK-16, AK-20) ---
f=$AGENTS_DIR/tech-lead.md
agent_common tech-lead
case $(fm_line "$f" tools) in *Bash*) ok "AK-15 tech-lead: Bash in tools" ;; *) fail "AK-15 tech-lead: Bash in tools" ;; esac
case $(fm_line "$f" disallowedTools) in *Bash*) fail "AK-15 tech-lead: Bash nicht in disallowedTools" ;; *) ok "AK-15 tech-lead: Bash nicht in disallowedTools" ;; esac
case $(fm_line "$f" tools) in *Write*) fail "AK-22 tech-lead: kein Write in tools" ;; *) ok "AK-22 tech-lead: kein Write in tools" ;; esac
has "AK-15 tech-lead: PreToolUse-Hook" "$f" 'PreToolUse:'
has "AK-15 tech-lead: Matcher Bash" "$f" 'matcher: Bash'
has "AK-15 tech-lead: Hook say-only.sh" "$f" 'command: sh \.claude/hooks/say-only\.sh'
has "AK-20 tech-lead: Task nennt Dateien" "$f" 'Dateien:'
has "AK-20 tech-lead: Task nennt hängt ab von" "$f" 'hängt ab von:'
has "AK-22 tech-lead: Akte nur per Edit, nie Write" "$f" 'nie `Write`'
has "Rückfall tech-lead: letzte Abschnittszeile fertig/blockiert" "$f" 'letzte Zeile.*(fertig|blockiert)'

# --- T14: orchestrator.md und merger.md (AK-16, 19, 20, 21, 22, 23) ---
f=$AGENTS_DIR/orchestrator.md
agent_common orchestrator 'scripts/team/say[.]sh'
lacks "AK-16 orchestrator: keine Ghostty-Kacheln mehr" "$f" 'Ghostty|Kachel'
case $(fm_line "$f" tools) in *Bash*) ok "orchestrator: Bash in tools" ;; *) fail "orchestrator: Bash in tools" ;; esac
has "AK-20 orchestrator: Welle" "$f" 'Welle'
has "AK-20 orchestrator: TEAM_MAX_SESSIONS" "$f" 'TEAM_MAX_SESSIONS'
has "AK-20 orchestrator: Dateien" "$f" 'Dateien'
has "AK-20 orchestrator: hängt ab von" "$f" 'hängt ab von'
has "AK-20 orchestrator: --instance" "$f" '--instance'
has "AK-20 orchestrator: abhängige Tasks erst nach Merge der Vorwelle" "$f" 'Merge der Vorwelle'
lacks "AK-20 orchestrator: 'immer nur eine schreibende Rolle' entfällt" "$f" '[Ii]mmer nur eine schreibende Rolle'
has "AK-19 orchestrator: max. 3 Fix-Runden" "$f" '[Mm]ax\. 3 Fix-Runden'
lacks "AK-19 orchestrator: kein 'max. 2'" "$f" '[Mm]ax\. 2'
has "AK-22 orchestrator: legt den Abschnitt vor dem Start an" "$f" 'Abschnitt.*vor dem Start'
has "AK-22 orchestrator: Abschnitt mit Instanz" "$f" '## <rolle>-<n>'
has "AK-23 orchestrator: 2 min blocked → Mensch" "$f" '2 min.*blocked|blocked.*2 min'
has "Rückfall orchestrator: herdr agent wait" "$f" 'herdr agent wait'
has "Rückfall orchestrator: Akte lesen" "$f" 'Akte'
has "AK-17 orchestrator: make dev" "$f" 'make dev'
has "R3 orchestrator: unbenannte Panes nach Neustart" "$f" 'unbenannte Panes'
has "AK-18 orchestrator: Kopfzeile ist keine Freigabe" "$f" '\[talker von'

f=$AGENTS_DIR/merger.md
agent_common merger
has "AK-21 merger: Task-Reihenfolge" "$f" 'Task-Reihenfolge'
has "AK-21 merger: verify:fast nach jedem Merge" "$f" 'nach jedem Merge'
has "AK-21 merger: Abbruch bei Konflikt" "$f" 'git merge --abort'
has "AK-21 merger: blockiert bei Konflikt" "$f" 'blockiert'
has "AK-21 merger: keine eigene Konfliktlösung" "$f" 'keine eigene Konfliktlösung'
lacks "AK-21 merger: löst keine Konflikte auf" "$f" 'mechanische Konflikte|auflösen'
has "AK-22 merger: Abschnitt nur per Edit" "$f" 'nie `Write`'

# --- T15: product-owner, architect, rust-developer, test-engineer (AK-16, AK-22) ---
akte_checks() { # <rolle>
  f=$AGENTS_DIR/$1.md
  has "AK-22 $1: Akte nur per Edit, nie Write" "$f" 'nie `Write`'
  has "AK-22 $1: Abschnitt mit Instanz" "$f" "## $1-<n>"
  has "Rückfall $1: letzte Abschnittszeile fertig/blockiert" "$f" 'letzte Zeile.*(fertig|blockiert)'
}
for r in product-owner architect rust-developer test-engineer; do
  agent_common "$r"
  akte_checks "$r"
done
has "AK-18 product-owner: Kopfzeile ist keine Freigabe" "$AGENTS_DIR/product-owner.md" '\[talker von .*keine Freigabe'

th_summary
