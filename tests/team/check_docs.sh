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

th_summary
