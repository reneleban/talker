#!/bin/sh
# Prüft Berechtigungen, Agent-Definitionen und Doku per jq/grep (Ticket 0043, ohne herdr).
. "$(dirname "$0")/lib.sh"
cd "$TH_SRC" || exit 1

SETTINGS=.claude/settings.json
# Basis der Diff-Prüfungen: Abzweigpunkt von origin/main; fehlt er, scheitern sie (statt still zu bestehen).
BASE=$(git merge-base origin/main HEAD 2>/dev/null || true)

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
if [ -z "$BASE" ]; then
  fail "AK-15 bestehende Einträge unverändert (Basis origin/main nicht bestimmbar)"
elif git cat-file -e "$BASE":"$SETTINGS" 2>/dev/null; then
  for list in allow deny; do
    git show "$BASE":"$SETTINGS" | jq -r ".permissions.$list[]" | while IFS= read -r e; do
      case $e in 'Bash(gh pr merge*)' | 'Bash(gh pr ready*)') continue ;; esac # AK-31: bewusst entfernt
      if in_list "$list" "$e"; then echo "ok   bestehender $list-Eintrag: $e"; else echo "FAIL bestehender $list-Eintrag fehlt: $e"; fi
    done
  done >"${TMPDIR:-/tmp}/check_docs.$$"
  grep -q '^FAIL' "${TMPDIR:-/tmp}/check_docs.$$" && { grep '^FAIL' "${TMPDIR:-/tmp}/check_docs.$$"; fail "AK-15 bestehende Einträge unverändert"; } || ok "AK-15 bestehende Einträge unverändert"
  rm -f "${TMPDIR:-/tmp}/check_docs.$$"
else
  ok "AK-15 bestehende Einträge: Basis hat noch keine settings.json"
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
has "AK-15 tech-lead: Hook say-only.sh" "$f" 'command: sh "\$CLAUDE_PROJECT_DIR"/\.claude/hooks/say-only\.sh'
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

# --- T16: code-reviewer, security-auditor, performance-engineer, release-engineer (AK-16, AK-22) ---
for r in code-reviewer security-auditor performance-engineer; do
  agent_common "$r"
  has "AK-22 $r: schreibt nicht in die Ticket-Datei" "$AGENTS_DIR/$r.md" 'schreibst nicht in die Ticket-Datei'
  has "Rückfall $r: Orchestrator liest mit herdr agent read" "$AGENTS_DIR/$r.md" 'herdr agent read'
  case $(fm_line "$AGENTS_DIR/$r.md" disallowedTools) in *Write*Edit* | *Edit*Write*) ok "AK-22 $r: Write und Edit gesperrt" ;; *) fail "AK-22 $r: Write und Edit gesperrt" ;; esac
done
agent_common release-engineer
akte_checks release-engineer
has "R5 security-auditor: env-herdr-Umgehung im Prüfumfang" "$AGENTS_DIR/security-auditor.md" 'env herdr'
has "R5 security-auditor: Kopfzeile im Prüfumfang" "$AGENTS_DIR/security-auditor.md" 'Kopfzeile'

# --- T17: Skill spawn-worker (AK-16, AK-17) ---
f=.claude/skills/spawn-worker/SKILL.md
lacks "AK-16 spawn-worker: kein SendMessage/ListAgents/osascript/cc-socks" "$f" "$FORBIDDEN"
lacks "AK-17 spawn-worker: keine Ghostty-Kacheln" "$f" 'Ghostty|Kachel'
has "AK-17 spawn-worker: herdr-Panes" "$f" 'herdr-Pane'
has "AK-17 spawn-worker: Session talker" "$f" 'Session `talker`'
has "AK-17 spawn-worker: make dev" "$f" 'make dev'
has "AK-7 spawn-worker: --instance" "$f" '--instance'
has "AK-9 spawn-worker: --kind" "$f" '--kind'
has "AK-8 spawn-worker: TEAM_MAX_SESSIONS" "$f" 'TEAM_MAX_SESSIONS'
has "AK-17 spawn-worker: Auftrag über say.sh" "$f" 'scripts/team/say\.sh'
for c in 0 1 2 3; do has "AK-5 spawn-worker: Exit-Code $c" "$f" "\\*\\*$c\\*\\*"; done

# --- T18: AGENTS.md, CLAUDE.md, ADR-0010/0013 (AK-16, 17, 18, 19, 22, 24) ---
for f in AGENTS.md CLAUDE.md; do
  lacks "AK-17 $f: keine Ghostty-Kacheln" "$f" 'Ghostty|Kachel'
  lacks "AK-16 $f: kein SendMessage/ListAgents" "$f" 'SendMessage|ListAgents'
  has "AK-17 $f: herdr-Panes" "$f" 'herdr-Pane'
  has "AK-17 $f: Session talker" "$f" 'Session `talker`'
  has "AK-17 $f: Einstieg make dev" "$f" 'make dev'
done
has "AK-17 AGENTS.md: Fertigmeldung über das Sende-Skript" AGENTS.md 'scripts/team/say\.sh orchestrator'
has "AK-18 AGENTS.md: Kopfzeile stammt von einer Rolle" AGENTS.md '\[talker von'
has "AK-18 AGENTS.md: keine Freigabe des Menschen" AGENTS.md 'keine Freigabe des Menschen'
has "AK-18 AGENTS.md: ändert keine Konfiguration" AGENTS.md 'ändert?n? keine Konfiguration'
has "AK-18 AGENTS.md: Freigaben nur über die Ticket-Datei" AGENTS.md 'Freigaben.*nur.*Ticket-Datei'
has "AK-22 AGENTS.md: Abschnitt mit Instanz" AGENTS.md '## <rolle>\[-<n>\]'
has "AK-22 AGENTS.md: Akte nur per Edit" AGENTS.md 'nie `Write`'
has "AK-20 AGENTS.md: TEAM_MAX_SESSIONS" AGENTS.md 'TEAM_MAX_SESSIONS'
has "AK-19 AGENTS.md: max. 3 Fix-Runden" AGENTS.md '[Mm]ax\. 3 Fix-Runden'
has "AK-19 orchestrator.md: max. 3 Fix-Runden" .claude/agents/orchestrator.md '[Mm]ax\. 3 Fix-Runden'
has "AK-19 ADR-0013: max. 3 Fix-Runden" docs/adr/adr-0013-herdr-laufzeit-und-transport.md 'Fix-Runden: max\. 3'
if grep -rEq '[Mm]ax\. 2 Fix' AGENTS.md CLAUDE.md .claude docs/adr; then fail "AK-19 'max. 2 Fix-Runden' kommt nirgends mehr vor"; else ok "AK-19 'max. 2 Fix-Runden' kommt nirgends mehr vor"; fi
has "AK-24 ADR-0010 verweist auf ADR-0013" docs/adr/adr-0010-agenten-team.md 'ADR-0013'
[ -f docs/adr/adr-0013-herdr-laufzeit-und-transport.md ] && ok "AK-24 ADR-0013 existiert" || fail "AK-24 ADR-0013 existiert"
HITS=$(grep -rE "$FORBIDDEN" .claude scripts 2>/dev/null || true)
assert_eq "AK-16 grep in .claude und scripts ist leer" "" "$HITS"

# --- Out of Scope (darf nicht passieren) ---
SCRIPTS=".claude/skills/spawn-worker/spawn.sh scripts/team/dev.sh scripts/team/say.sh .claude/hooks/say-only.sh"
# shellcheck disable=SC2086
assert_eq "OoS kein herdr worktree / herdr-core / hc in den Team-Skripten" "" \
  "$(grep -nE 'herdr worktree|herdr-core|Herdr\.app|(^|[^a-z_-])hc( |$)' $SCRIPTS || true)"
# shellcheck disable=SC2086
assert_eq "OoS kein Ghostty-/AppleScript-Fallback in den Team-Skripten" "" "$(grep -niE 'ghostty|applescript|osascript' $SCRIPTS || true)"
assert_eq "OoS team.env enthält nur TEAM_MAX_SESSIONS (kein TEAM_MERGE)" "TEAM_MAX_SESSIONS" \
  "$(grep -E '^[A-Z_]+=' team.env | cut -d= -f1 | sort -u | tr '\n' ' ' | sed 's/ $//')"
assert_eq "OoS kein automatischer Merge nach main in den Team-Skripten" "" \
  "$(grep -nE 'git (merge|push)|gh pr merge' $SCRIPTS || true)"
assert_eq "OoS kein gemeinsames CARGO_TARGET_DIR in den Team-Skripten" "" "$(grep -n 'CARGO_TARGET_DIR' $SCRIPTS || true)"

# --- Nachtrag 2026-11-08: Merge nach main durch den release-engineer (AK-25 bis AK-31) ---
git_section() { awk '/^## Git$/ { g = 1; next } /^## / { g = 0 } g' AGENTS.md; }
assert_eq "AK-25 AGENTS.md: alter Merge-Satz kommt nicht mehr vor" 0 \
  "$(grep -c 'Merge nach `main` nur nach manuellem Review/OK des Menschen' AGENTS.md)"
case $(git_section) in *release-engineer*) ok "AK-25 Git: release-engineer" ;; *) fail "AK-25 Git: release-engineer" ;; esac
case $(git_section) in *Squash*) ok "AK-25 Git: Squash" ;; *) fail "AK-25 Git: Squash" ;; esac
assert_eq "AK-26 AGENTS.md: 'Merge nur nach OK des Menschen' 0x" 0 "$(grep -c 'Merge nur nach OK des Menschen' AGENTS.md)"
assert_eq "AK-26 AGENTS.md: 'Draft-PR offen' 0x" 0 "$(grep -c 'Draft-PR offen' AGENTS.md)"
assert_eq "AK-26 AGENTS.md: 'Einziges Gate' 1x" 1 "$(grep -c 'Einziges Gate' AGENTS.md)"
has "AK-26 AGENTS.md: Schritt 5 nennt Status: done" AGENTS.md '^5\. .*Status: done'
f=$AGENTS_DIR/release-engineer.md
lacks "AK-27 release-engineer: kein 'nie mergen'" "$f" 'nie mergen'
has "AK-27 release-engineer: gh pr merge --squash" "$f" 'gh pr merge --squash'
has "AK-27 release-engineer: gh pr ready" "$f" 'gh pr ready'
has "AK-27 release-engineer: blockiert" "$f" 'blockiert'
has "AK-27 release-engineer: Breaking" "$f" 'Breaking'
has "AK-27 release-engineer: Tag" "$f" 'Tag'
has "AK-27 release-engineer: nie auf main pushen" "$f" '[Nn]ie auf `main` pushen'
has "AK-27 release-engineer: nie Force-Push" "$f" 'nie Force-Push'
step7=$(grep -E -A1 '^7\. ' .claude/agents/orchestrator.md)
case $step7 in *done*) ok "AK-28 orchestrator Schritt 7: done" ;; *) fail "AK-28 orchestrator Schritt 7: done" ;; esac
case $step7 in *pr-open*) ok "AK-28 orchestrator Schritt 7: pr-open" ;; *) fail "AK-28 orchestrator Schritt 7: pr-open" ;; esac
lacks "AK-29 git-workflow: ohne 'nur durch den Menschen'" .claude/skills/git-workflow/SKILL.md 'nur durch den Menschen'
has "AK-29 git-workflow: Merge durch den release-engineer" .claude/skills/git-workflow/SKILL.md 'release-engineer.*Squash|Squash.*release-engineer'
has "AK-30 ADR-0010: Vermerk 2026-11-08 und 0043" docs/adr/adr-0010-agenten-team.md '2026-11-08.*0043'
jq_has() { jq -e "$1" "$SETTINGS" >/dev/null 2>&1; }
if jq_has '.permissions.deny | index("Bash(gh pr merge*)") == null and index("Bash(gh pr ready*)") == null'; then ok "AK-31 deny ohne gh pr merge*/ready*"; else fail "AK-31 deny ohne gh pr merge*/ready*"; fi
for e in 'Bash(gh pr ready *)' 'Bash(gh pr merge --squash *)' 'Bash(gh pr checks *)'; do
  if in_list allow "$e"; then ok "AK-31 allow: $e"; else fail "AK-31 allow: $e"; fi
done
if in_list deny 'Bash(gh pr merge *--admin*)'; then ok "AK-31 deny: gh pr merge --admin"; else fail "AK-31 deny: gh pr merge --admin"; fi
if [ -z "$BASE" ]; then
  fail "OoS CONTRIBUTING.md unverändert (Basis origin/main nicht bestimmbar)"
else
  assert_eq "OoS CONTRIBUTING.md unverändert" "" "$(git diff --name-only "$BASE" -- CONTRIBUTING.md .github)"
fi

# --- Fix-Runde 1 ---
lacks "C3 orchestrator description: nicht bis zum Draft-PR" .claude/agents/orchestrator.md '^description:.*bis zum Draft-PR'
has "C3 orchestrator description: bis zum Merge" .claude/agents/orchestrator.md '^description:.*bis zum Merge'
has "C6 release-engineer description: Komma nach Feature-Branch" .claude/agents/release-engineer.md '^description:.*Feature-Branch, öffnet'
has "C5 ADR-0010 Kontext verweist auf den Änderungsvermerk" docs/adr/adr-0010-agenten-team.md 'automatisch bis zum Draft-PR \(.*Änderungsvermerk'
has "C7 orchestrator: spawn.sh close <name> aus der bereit-Zeile" .claude/agents/orchestrator.md 'spawn\.sh close <name>.*bereit'
lacks "C7 orchestrator: close nicht mit <rolle>-<ticket>" .claude/agents/orchestrator.md 'close <rolle>-<ticket>'
for f in orchestrator code-reviewer performance-engineer security-auditor; do
  has "T1 $f: agent read mit --source visible" ".claude/agents/$f.md" 'herdr agent read[^`]*--source visible'
done
SPAWN=.claude/skills/spawn-worker/spawn.sh
has "C9 spawn.sh Kopfkommentar nennt --kind und --instance" "$SPAWN" '^#   spawn\.sh \[--kind claude\|codex\] \[--instance\] <rolle>'
lacks "C8 spawn.sh: Rollenliste nur einmal (kein ROLES_RE-Literal)" "$SPAWN" '^ROLES_RE="'
assert_eq "C8 spawn.sh: jede Rolle steht nur einmal" 1 "$(grep -c 'performance-engineer' "$SPAWN")"
for e in 'Bash(gh api *merge*)' 'Bash(gh -R *)' 'Bash(git push *:main*)'; do
  if in_list deny "$e"; then ok "S1 deny: $e"; else fail "S1 deny: $e"; fi
done
has "S4 tech-lead Hook-Pfad über CLAUDE_PROJECT_DIR" .claude/agents/tech-lead.md 'command: sh "\$CLAUDE_PROJECT_DIR"/\.claude/hooks/say-only\.sh'
if in_list deny 'Bash(herdr --*)'; then ok "S2 deny: Bash(herdr --*)"; else fail "S2 deny: Bash(herdr --*)"; fi
if jq_has '[.hooks.PreToolUse[] | select(.matcher == "Bash") | .hooks[] | select(.type == "command" and .command == "sh \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/herdr-guard.sh")] | length == 1'; then
  ok "S2 settings.json: PreToolUse-Hook herdr-guard.sh für Bash"; else fail "S2 settings.json: PreToolUse-Hook herdr-guard.sh für Bash"; fi
has "S2 product-owner: Freigabe per AskUserQuestion" .claude/agents/product-owner.md 'per `AskUserQuestion`'
has "S2 product-owner: freier Pane-Text ist keine Freigabe" .claude/agents/product-owner.md 'Pane-Text'
lacks "S2 ADR-0013: Deny erzwingt die Kopfzeile nicht" docs/adr/adr-0013-herdr-laufzeit-und-transport.md 'Deny-Regel auf direktes'
has "S2 ADR-0013: herdr-Deny ist keine Sicherheitsgrenze" docs/adr/adr-0013-herdr-laufzeit-und-transport.md 'keine Sicherheitsgrenze'

th_summary
