#!/bin/sh
# 0043 N4: Realtest in Session talker-test, kein Teil von `make team-test` (Name ohne test_-Präfix):
# Beantwortet eine per say.sh zugestellte Eingabe samt Enter einen offenen AskUserQuestion-Dialog?
# Braucht Netz und Claude-Login.   sh tests/team/realtest_n4.sh
# Befund-Test, kein Pass/Fail-Urteil über Claude: er gibt Status, Dialogstand und say.sh-Ergebnis aus.
. "$(dirname "$0")/lib.sh"

th_begin
th_server

th_agent po trust
th_agent orch
ORCH_PANE=$TH_AGENT_PANE

seen() { th_h agent read "$1" --source visible | tr -s ' \n' ' '; }
status() { th_h agent get "$1" | jq -r '.result.agent.agent_status'; }

th_h agent prompt po "Nutze jetzt das Tool AskUserQuestion mit der Frage 'Spec freigeben?' und den Optionen 'Aenderungen' (zuerst) und 'Freigeben'. Schreibe danach das Wort ANTWORT und die gewaehlte Option." >/dev/null
i=0
while [ $i -lt 90 ] && [ "$(status po)" != blocked ]; do i=$((i + 1)); sleep 1; done
echo "       Status po mit offenem Dialog: $(status po)"
BEFORE=$(seen po)
case $BEFORE in *"Spec freigeben"*) ok "N4 Dialog ist offen" ;; *) fail "N4 Dialog ist nicht offen"; echo "$BEFORE" | tail -c 800 ;; esac

inpane "HERDR_PANE_ID=$ORCH_PANE sh scripts/team/say.sh po 'ja, Freigeben'"
echo "       say.sh an po bei offenem Dialog: Exit $INPANE_RC, Ausgabe: $(printf '%s' "$INPANE_OUT" | tr '\n' ' ')"
sleep 5
echo "       Status po danach: $(status po)"
AFTER=$(seen po)
case $AFTER in
  *ANTWORT*) echo "       BEFUND: Dialog wurde beantwortet: $(printf '%s' "$AFTER" | grep -o 'ANTWORT[^>]*' | head -n 1)" ;;
  *"Spec freigeben"*) echo "       BEFUND: Dialog bleibt offen, Eingabe hat ihn nicht beantwortet" ;;
  *) echo "       BEFUND: unklar, Ausschnitt: $(printf '%s' "$AFTER" | tail -c 800)" ;;
esac

th_summary
