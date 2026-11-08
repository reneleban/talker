#!/bin/sh
# T19 / AK-14: Realtest Claude + Codex über say.sh im Wegwerf-Repo (Session talker-test). Kein Teil von
# `make team-test` (Name ohne test_-Präfix): braucht Netz, Claude- und Codex-Login, ggf. Mensch am Terminal.
#   sh tests/team/realtest_ak14.sh
# Orchestrator-Stand-in = Claude-Haiku-Agent `orchestrator`; Aufträge gehen per say.sh mit der Pane des
# Stand-ins als Absender (wie test_say.sh). Codex-Scheitern ist ein dokumentierter Befund, kein Testfehler (rc 0).
. "$(dirname "$0")/lib.sh"

th_begin
cp "$TH_SRC/.claude/settings.json" "$TH_REPO/.claude/settings.json"
git -C "$TH_REPO" add -A
git -C "$TH_REPO" commit -q -m "settings"
th_server
th_autotrust

SP='sh .claude/skills/spawn-worker/spawn.sh'
inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"
th_agent orchestrator
ORCH_PANE=$TH_AGENT_PANE

seen() { th_h agent read "$1" --source recent-unwrapped --lines 300 | tr -s ' \n' ' '; }
wait_for() { # <agent> <text> <sekunden> → $SEEN
  i=0
  while [ $i -lt "$3" ]; do
    SEEN=$(seen "$1")
    case $SEEN in *"$2"*) return 0 ;; esac
    i=$((i + 1)); sleep 1
  done
  return 1
}
send() { inpane "HERDR_PANE_ID=$ORCH_PANE sh scripts/team/say.sh $1 '$2'"; }

# --- Claude-Rolle ---
inpane "$SP rust-developer 0001 chore/0001-x"
assert_eq "Claude-Rolle startet: Exit 0" 0 "$INPANE_RC"
send rust-developer-0001 "Fuehre mit dem Bash-Tool genau diesen Befehl aus: sh scripts/team/say.sh orchestrator '\\''fertig rust-developer 0001: kein Commit - claude-ok'\\'' . Danach nichts mehr."
assert_eq "Auftrag an Claude zugestellt: Exit 0" 0 "$INPANE_RC"
if wait_for orchestrator "[talker von rust-developer-0001] fertig rust-developer 0001: kein Commit - claude-ok" 120; then
  ok "AK-14 Claude: fertig-Meldung kommt mit Kopfzeile beim Orchestrator an"
else
  fail "AK-14 Claude: fertig-Meldung kommt nicht an"
  echo "--- rust-developer-0001:"; seen rust-developer-0001 | tail -c 1500
fi

# --- Grenzfall B8: Nachricht mitten im Turn des Orchestrator-Stand-ins ---
# Stand-in bekommt einen Auftrag mit sleep 20 (Turn läuft), währenddessen meldet die Claude-Rolle erneut.
th_h agent prompt orchestrator "Fuehre mit dem Bash-Tool aus: sleep 20 . Antworte danach genau mit dem Wort ZWEI." >/dev/null
sleep 5
echo "       Status Stand-in vor der Mitten-im-Turn-Nachricht: $(th_h agent get orchestrator | jq -r '.result.agent.agent_status')"
send rust-developer-0001 "Fuehre mit dem Bash-Tool genau diesen Befehl aus: sh scripts/team/say.sh orchestrator '\\''MITTENDRIN-DREI'\\'' . Danach nichts mehr."
if wait_for orchestrator "MITTENDRIN-DREI" 120; then ok "B8 Meldung mitten im Turn kommt beim Orchestrator an"; else fail "B8 Meldung mitten im Turn kommt nicht an"; fi
th_h agent wait orchestrator --until idle --timeout 90000 >/dev/null 2>&1
SEEN=$(seen orchestrator)
case $SEEN in
  *ZWEI*) echo "       B8-Befund: laufender Auftrag (ZWEI) wurde beantwortet" ;;
  *) echo "       B8-Befund: Antwort ZWEI fehlt, laufender Auftrag wurde verdraengt" ;;
esac
echo "       B8-Ausschnitt: $(printf '%s' "$SEEN" | tail -c 600)"

# --- Codex-Rolle (Befund, kein Testfehler) ---
if command -v codex >/dev/null 2>&1; then
  inpane "$SP --kind codex security-auditor 0001 chore/0001-x"
  echo "       Codex-Start: Exit $INPANE_RC, $(printf '%s' "$INPANE_OUT" | tail -n 2 | tr '\n' ' ')"
  echo "       Codex-Status: $(th_h agent get security-auditor-0001 | jq -r '.result.agent.agent_status')"
  send security-auditor-0001 "Fuehre aus: sh scripts/team/say.sh orchestrator '\\''fertig security-auditor 0001: kein Commit - codex-ok'\\'' . Schreibe ausserdem die Datei $TH_TMP/akte-codex.md mit dem Inhalt test. Danach nichts mehr."
  echo "       Auftrag an Codex: Exit $INPANE_RC $(printf '%s' "$INPANE_OUT" | tail -n 1)"
  if wait_for orchestrator "[talker von security-auditor-0001] fertig security-auditor 0001: kein Commit - codex-ok" 90; then
    ok "AK-14 Codex: fertig-Meldung kommt mit Kopfzeile beim Orchestrator an"
  else
    echo "       BEFUND Codex: fertig-Meldung kam nicht an. Status: $(th_h agent get security-auditor-0001 | jq -r '.result.agent.agent_status')"
    echo "       Codex-Ausschnitt: $(seen security-auditor-0001 | tail -c 1500)"
  fi
  [ -f "$TH_TMP/akte-codex.md" ] && echo "       Codex-Sandbox: Schreiben ausserhalb des Arbeitsorts ERLAUBT" || echo "       Codex-Sandbox: Schreiben ausserhalb des Arbeitsorts nicht erfolgt (A6)"
else
  skip "Codex nicht installiert"
fi

th_summary
