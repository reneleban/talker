# Streaming Dictation — fachliche Anforderungen (EARS)

Feature-Bereich: Live-Diktat mit zwei Stufen (Einfüge-Verhalten, Korrektur).
Muster und ID-Konvention: [`README.md`](README.md). Entscheidung:
[ADR-0009](../adr/adr-0009-streaming-stufen.md).

## Einfüge-Verhalten (Stufe 1)

- **REQ-STREAM-001**: Das System soll drei Einfüge-Verhalten anbieten — Am Ende,
  Satzweise, Wortgruppenweise — mit Am Ende als Default.
- **REQ-STREAM-002**: FALLS die Config kein `insert_mode` enthält, DANN soll das
  System Am Ende verwenden (Bestandsconfigs bleiben gültig).
- **REQ-STREAM-003**: WO Am Ende aktiv ist, soll das System das Audio nur im
  Batch-Pfad verarbeiten, ohne Streaming-Worker.
- **REQ-STREAM-004**: WO Satzweise aktiv ist, soll das System den bis zur
  Sprechpause erkannten Satz einfügen, sobald die Pause `endpoint_silence_ms`
  erreicht.
- **REQ-STREAM-005**: WO Wortgruppenweise aktiv ist, soll das System nur
  Wörter einfügen, die der Recognizer als stabil meldet.
- **REQ-STREAM-006**: WENN die PTT-Taste im Live-Modus losgelassen wird, soll
  das System den noch nicht eingefügten Rest der Utterance einfügen.
- **REQ-STREAM-007**: Das System soll bereits eingefügten Text nicht zurücknehmen
  oder ersetzen.
- **REQ-STREAM-008**: FALLS `endpoint_silence_ms` fehlt, DANN soll das System
  einen Default-Wert verwenden.

## Korrektur (Stufe 2)

- **REQ-STREAM-009**: Das System soll je Einfüge-Verhalten nur folgende
  Cleanup-Modi erlauben — Am Ende: alle vier; Satzweise: Roh, Geschäftlich,
  Natürlich; Wortgruppenweise: Roh.
- **REQ-STREAM-010**: FALLS der konfigurierte Cleanup-Modus (oder eine
  App-Regel) beim aktiven Einfüge-Verhalten nicht erlaubt ist, DANN soll das
  System den nächsten erlaubten Modus verwenden (Roh bei Wortgruppenweise).
- **REQ-STREAM-011**: WO Satzweise mit Geschäftlich oder Natürlich aktiv ist,
  soll das System jeden Satz vor dem Einfügen einzeln korrigieren.

## Laden und Fallback

- **REQ-STREAM-012**: Das System soll beim App-Start nur die Komponenten laden,
  die die Konfiguration benötigt.
- **REQ-STREAM-013**: FALLS im Live-Modus der Streaming-Recognizer nicht
  verfügbar ist, DANN soll das System auf Am Ende zurückfallen und den Hinweis
  nur in den Settings anzeigen.

## Settings

- **REQ-STREAM-014**: Die Settings sollen Einfüge-Verhalten und Korrektur als
  zwei Auswahlfelder anbieten; nicht erlaubte Modi sollen ausgegraut mit
  Hinweis erscheinen.
- **REQ-STREAM-015**: WENN der Nutzer ein Einfüge-Verhalten wählt, das den
  gewählten Cleanup-Modus nicht erlaubt, soll das System auf den nächsten
  erlaubten Modus umschalten.
