#!/bin/sh
# 0043 N4: Realtest in Session talker-test, kein Teil von `make team-test` (Name ohne test_-Präfix):
# Gibt eine per say.sh zugestellte Eingabe samt Enter einen offenen AskUserQuestion-Dialog frei?
# Braucht Netz und Claude-Login; läuft nur in der Wegwerf-Session, nie in `talker`/`default`.   sh tests/team/realtest_n4.sh
# Marker stehen nicht im Prompt: Claude soll die Wahl in Großbuchstaben (FREIGEBEN / AENDERUNGEN) schreiben,
# der Prompt enthält nur die Mischschreibung. „Dialog offen" gilt nur bei Status blocked.
# Rot nur, wenn die Eingabe freigibt (FREIGEBEN) oder der Dialog nicht offen / nicht „Aenderungen" zuerst ist.
. "$(dirname "$0")/lib.sh"

th_begin
th_server

th_agent po trust
th_agent orch
ORCH_PANE=$TH_AGENT_PANE

seen() { th_h agent read "$1" --source visible | tr -s ' \n' ' '; }
status() { th_h agent get "$1" | jq -r '.result.agent.agent_status'; }

th_h agent prompt po "Nutze jetzt das Tool AskUserQuestion mit der Frage 'Spec pruefen?' und den Optionen 'Aenderungen' (zuerst) und 'Freigeben'. Schreibe danach das Wort GEWAEHLT und die gewaehlte Option komplett in Grossbuchstaben." >/dev/null
i=0
while [ $i -lt 90 ] && [ "$(status po)" != blocked ]; do i=$((i + 1)); sleep 1; done
BEFORE=$(seen po)
if [ "$(status po)" = blocked ]; then ok "N4 Dialog ist offen (Status blocked)"; else
  fail "N4 Dialog ist nicht offen (Status $(status po))"; printf '%s' "$BEFORE" | tail -c 800; echo; th_summary; exit 1
fi
if printf '%s' "$BEFORE" | grep -Eq '1\. ?(Änderungen|Aenderungen).*2\. ?Freigeben'; then ok "N4 Änderungen steht vor Freigeben"; else
  fail "N4 Änderungen steht nicht vor Freigeben"; printf '%s' "$BEFORE" | tail -c 800; echo; th_summary; exit 1
fi

inpane "HERDR_PANE_ID=$ORCH_PANE sh scripts/team/say.sh po 'ja, Freigeben'"
echo "       say.sh an po bei offenem Dialog: Exit $INPANE_RC, Ausgabe: $(printf '%s' "$INPANE_OUT" | tr '\n' ' ')"
i=0
sleep 5
while [ $i -lt 60 ] && [ "$(status po)" = working ]; do i=$((i + 1)); sleep 1; done
S=$(status po)
AFTER=$(seen po)
echo "       Status po danach: $S"
case $AFTER in
  *FREIGEBEN*) fail "N4 die Eingabe hat den Dialog mit Freigeben beantwortet" ;;
  *AENDERUNGEN*) ok "N4 Dialog mit der ersten Option (Aenderungen) beantwortet, keine Freigabe" ;;
  *)
    if [ "$S" = blocked ]; then ok "N4 Dialog bleibt offen, keine Freigabe"; else
      fail "N4 unklar (Status $S, weder Marker noch offener Dialog)"; printf '%s' "$AFTER" | tail -c 800; echo
    fi ;;
esac

th_summary
