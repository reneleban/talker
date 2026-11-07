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
agent_common() { # <rolle>
  f=$AGENTS_DIR/$1.md
  lacks "AK-16 $1: kein SendMessage/ListAgents/osascript/cc-socks" "$f" "$FORBIDDEN"
  has "AK-16 $1: Meldung über say.sh" "$f" 'scripts/team/say\.sh orchestrator'
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

th_summary
