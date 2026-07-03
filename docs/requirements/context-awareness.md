# Kontext-Awareness — Fachliche Anforderungen (EARS)

Feature-Bereich: automatische Cleanup-Modus-Wahl je fokussierter App
(Selektor-Schicht über den bestehenden Cleanup-Modi, kein neuer
Cleanup-Layer). Muster-Erklärung und ID-Konvention:
[`README.md`](README.md). Begriffe: [`CONTEXT.md`](../../CONTEXT.md).

## Modus-Auflösung

- **REQ-CTX-001**: WO `context_aware_enabled` deaktiviert ist, soll das
  System den manuell konfigurierten `cleanup_mode` als effektiven Modus
  verwenden. (src/pipeline.rs:`resolve_mode`; Ticket-0026 AK3)
- **REQ-CTX-002**: WO `context_aware_enabled` aktiviert ist UND die
  frontmost bundle-id einer Regel in `context_rules` entspricht, soll das
  System den in dieser Regel hinterlegten Cleanup-Modus als effektiven
  Modus verwenden. (src/pipeline.rs:`resolve_mode`; Ticket-0026 AK3)
- **REQ-CTX-003**: FALLS `context_aware_enabled` aktiviert ist UND keine
  Regel auf die frontmost bundle-id passt, DANN soll das System auf den
  manuell konfigurierten `cleanup_mode` zurückfallen. (src/pipeline.rs:`resolve_mode`)
- **REQ-CTX-004**: FALLS die frontmost App nicht ermittelt werden konnte
  (kein bundle-id-Wert), DANN soll das System unabhängig vom
  Feature-Zustand auf den manuell konfigurierten `cleanup_mode`
  zurückfallen. (src/pipeline.rs:`resolve_mode`; Ticket-0026 AK3)
- **REQ-CTX-005**: WENN eine Aufnahme beendet wird (PTT-Release) und der
  Text zur Verarbeitung übergeben wird, soll das System den beim
  zugehörigen PTT-Press aufgelösten Cleanup-Modus für diese Utterance
  anwenden, nicht den zum Verarbeitungszeitpunkt gültigen Modus.
  (src/pipeline.rs:`PttSession::press`/`release`; Ticket-0026 AK4)

## Regel-Verwaltung

- **REQ-CTX-006**: Das System soll Kontext-Regeln als Liste von
  bundle-id/Cleanup-Modus-Paaren verwalten, wobei jede bundle-id höchstens
  einmal vorkommt. (src/config.rs:`context_rules`; src/ui.rs:`upsert_rule`)
- **REQ-CTX-007**: WENN über den App-Picker oder die Regel-Liste eine Regel
  für eine bundle-id angelegt wird, die bereits eine Regel hat, soll das
  System den Modus der bestehenden Regel aktualisieren statt eine zweite
  Regel für dieselbe bundle-id anzulegen. (src/ui.rs:`upsert_rule`;
  Ticket-0027 AK2/AK3)
- **REQ-CTX-008**: Das System soll bei der Modus-Zuordnung die erste (und
  einzige) zu einer bundle-id passende Regel verwenden — „erste Regel
  gewinnt" darf durch REQ-CTX-006/007 nie mehrdeutig werden.
  (src/pipeline.rs:`resolve_mode`; src/ui.rs:`upsert_rule`)
- **REQ-CTX-009**: WO die Kontext-Einstellungen angezeigt werden, soll das
  System zu jeder Regel den App-Klarnamen anzeigen, sofern die App aktuell
  läuft, sonst die bundle-id selbst. (src/ui.rs Kontext-Tab; Ticket-0027 AK3)
- **REQ-CTX-010**: Das System soll dem Nutzer erlauben, eine Regel über
  einen App-Picker aus den aktuell laufenden Apps anzulegen, sodass die
  bundle-id automatisch erfasst wird und nicht manuell bekannt sein muss.
  (src/ui.rs Kontext-Tab, App-Picker; Ticket-0027 AK3)
- **REQ-CTX-011**: WENN eine Regel in der Kontext-Einstellungen-Liste
  entfernt wird, soll das System sie aus `context_rules` löschen und die
  Änderung in der Config persistieren. (src/ui.rs Kontext-Tab; Ticket-0027 AK2)
- **REQ-CTX-012**: FALLS `context_rules` leer ist, DANN soll das System dies
  in der Kontext-Einstellungen-Liste sichtbar machen (kein mitgelieferter
  Default-Regelsatz) und die Modus-Auflösung wie in REQ-CTX-003 auf den
  manuellen Modus zurückfallen. (src/ui.rs Kontext-Tab; Ticket-0026 Grill-Entscheidung „KEIN mitgelieferter Regelsatz")

## Stabilität während der Aufnahme

- **REQ-CTX-013**: WENN PTT gedrückt wird, soll das System die
  bundle-id der frontmost App zu diesem Zeitpunkt erfassen (nicht erst
  beim Loslassen). (src/pipeline.rs:`PttSession::press`; Ticket-0026 AK1)
- **REQ-CTX-014**: WÄHREND eine Aufnahme läuft, soll das System den beim
  PTT-Press erfassten frontmost-Wert unverändert für die gesamte Utterance
  beibehalten, auch wenn der Nutzer währenddessen zu einer anderen App
  wechselt. (src/pipeline.rs:`PttSession` Feld `frontmost`, `release`; Ticket-0026 AK1)
- **REQ-CTX-015**: WÄHREND eine Aufnahme läuft (`Phase::Recording`), soll
  das Tray-Badge den für diese Utterance aufgelösten Cleanup-Modus
  anzeigen, nicht den manuell eingestellten `cleanup_mode`.
  (src/tray.rs:`recording_mode`, `mode_badge`; Ticket-0026 AK5)
