# Diagnostics — fachliche Anforderungen (EARS)

Feature-Bereich: Logging (Datei, Rotation, Detailgrad), Messzeile je Dictation, Debug-Mitschnitt,
Audioquelle für Tests, Latenz-Messreihe. Muster und ID-Konvention:
[`README.md`](README.md). Hintergrund:
[`docs/research/mikrofon-start-und-pre-roll.md`](../research/mikrofon-start-und-pre-roll.md).

## Logging

- **REQ-DIAG-001**: Das System soll alle Logzeilen über genau einen Logpfad in
  die Datei `~/Library/Logs/talker/talker.log` schreiben, unabhängig davon, ob
  `talker.app` aus Terminal, Finder oder als Login-Item gestartet wurde.
- **REQ-DIAG-002**: WENN die Logdatei 1 MB überschreitet, soll das System sie
  rotieren, höchstens 4 ältere Logdateien behalten und die älteste löschen
  (gesamt höchstens 5 MB).
- **REQ-DIAG-003**: Das System soll den Log-Detailgrad aus dem Config-Feld
  `log_level` (`error`, `warn`, `info`, `debug`) lesen, mit `info` als Default.
- **REQ-DIAG-004**: FALLS die Config kein `log_level` enthält, DANN soll das
  System `info` verwenden (Bestandsconfigs bleiben gültig).
- **REQ-DIAG-005**: WENN der Nutzer in den Settings den Detailgrad ändert, soll
  das System ihn ohne Neustart anwenden und in der Config speichern.
- **REQ-DIAG-006**: WO die Umgebungsvariable `TALKER_LOG` einen gültigen
  Detailgrad enthält, soll das System diesen für den laufenden Prozess statt
  `log_level` verwenden, und die Settings sollen die Auswahl sperren und
  „durch TALKER_LOG übersteuert" anzeigen.
- **REQ-DIAG-007**: FALLS `TALKER_LOG` einen ungültigen Wert enthält, DANN soll
  das System `log_level` aus der Config verwenden und eine Warnung loggen.
- **REQ-DIAG-008**: WENN der Nutzer in den Settings „Log im Finder zeigen"
  wählt, soll das System die Logdatei im Finder markieren, oder den
  Log-Ordner öffnen, falls die Datei noch nicht existiert.
- **REQ-DIAG-009**: Das System soll auf den Graden `info`, `warn` und `error`
  nichts pro Audio-Block oder pro Segment loggen; je Dictation entsteht dort
  genau eine Zeile (die Messzeile), zuzüglich Warnungen, Fehlern und
  Modell-Lebenszyklus-Ereignissen (geladen, gewechselt, fehlgeschlagen).
- **REQ-DIAG-010**: FALLS das Log-Verzeichnis nicht anlegbar oder die Logdatei
  nicht beschreibbar ist, DANN soll das System einen Hinweis auf stderr
  ausgeben und ohne Datei-Log weiterlaufen.
- **REQ-DIAG-011**: FALLS das Logging selbst fehlschlägt, DANN soll das System
  die Dictation ohne Panic und ohne Blockieren des Hot-Paths (Aufnahme → STT →
  Cleanup → Injection) fortsetzen.

## Messzeile

- **REQ-DIAG-012**: WENN eine Dictation endet (eingefügt oder verworfen), soll
  das System genau eine Messzeile auf Grad `info` loggen mit: laufender
  Diktat-Nummer, Taste→Mikro bereit, →erster Block, →erstes Signal, gehalten,
  aufgenommen, Loslassen→Text (STT, Cleanup), →eingefügt, Gerät (Name,
  Samplerate, Kanäle), Einfüge-Verhalten, Cleanup-Modus, Sprache, Kaltstart und
  Ergebnis.
- **REQ-DIAG-013**: Das System soll alle Zeitpunkte der Messzeile auf einer
  gemeinsamen Uhr relativ zum Tastendruck angeben.
- **REQ-DIAG-014**: Das System soll „erstes Signal" als ersten Audio-Block mit
  mindestens einem Sample ungleich 0 bestimmen.
- **REQ-DIAG-015**: WO ein Einfüge-Verhalten außer Am Ende aktiv ist, soll die
  Messzeile zusätzlich →erstes Segment und die Anzahl der Segmente enthalten,
  und Loslassen→Text sowie dessen Anteile STT und Cleanup sollen nur die
  Verarbeitung des Rests nach dem Loslassen messen; die Anteile zählen ab dem
  Ende des Audio-Stroms (`Feed::End`), sodass die Differenz zu Loslassen→Text
  den Rückstand der Spracherkennung beim Loslassen zeigt.
- **REQ-DIAG-016**: WENN während einer Dictation ein Modell geladen wurde, soll
  die Messzeile die Ladedauer je Modell enthalten.
- **REQ-DIAG-017**: FALLS ein Messpunkt nicht eingetreten ist (kein Block, kein
  Signal, verworfen, Fehler, Cleanup-Modus Roh), DANN soll das System den Wert
  als `—` ausgeben und die Messzeile trotzdem loggen.
- **REQ-DIAG-018**: Das System soll in keine Logzeile den Text eines Raw
  Transcript oder Cleaned Transcript schreiben.

## Debug-Mitschnitt

- **REQ-DIAG-019**: WO die Umgebungsvariable `TALKER_DEBUG_WAV_DIR` gesetzt
  ist, soll das System je Dictation zwei WAV-Dateien (32-bit Float) in dieses
  Verzeichnis schreiben: `<nr>-mic.wav` mit den unveränderten Audio-Blöcken ab
  dem ersten Block in Geräte-Samplerate und -Kanälen, und `<nr>-model.wav` mit
  exakt dem Audio, das das STT-Modell erhält (16 kHz mono, inklusive Vor- und
  Nachlauf-Stille, im Live-Modus inklusive der Stille nach jedem Reset).
- **REQ-DIAG-020**: Das System soll den Debug-Mitschnitt nicht im
  Audio-Callback auf Platte schreiben.
- **REQ-DIAG-021**: FALLS das Verzeichnis aus `TALKER_DEBUG_WAV_DIR` fehlt,
  DANN soll das System es anlegen; FALLS Anlegen oder Schreiben fehlschlägt,
  DANN soll das System eine Warnung loggen und die Dictation unverändert
  fortsetzen.
- **REQ-DIAG-022**: WÄHREND `TALKER_DEBUG_WAV_DIR` nicht gesetzt ist, soll das
  System kein Audio auf Platte schreiben (REQ-DICT-003).

## Audioquelle

- **REQ-DIAG-023**: Das System soll Audio in App und Tests über dieselbe
  austauschbare Audioquellen-Schnittstelle beziehen, mit einer Mikrofon-Quelle
  (cpal) für die App und einer WAV-Quelle für Tests und Messreihen.
- **REQ-DIAG-024**: Die WAV-Quelle soll eine WAV-Datei als Blöcke mit
  einstellbarer Samplerate, Kanalzahl, Blockgröße und Startverzögerung bis zum
  ersten Block liefern, wahlweise im Echtzeittakt oder ohne Wartezeit.

## Latenz-Messreihe

- **REQ-DIAG-025**: WENN `make latency-eval` läuft, soll das System jede
  Fixture aus `tests/fixtures/stt/` über die WAV-Quelle im Echtzeittakt N-mal
  (Default 10, plus ein nicht gezählter Aufwärmlauf) je Einfüge-Verhalten Am
  Ende und Satzweise und je Cleanup-Modus Roh und Natürlich verarbeiten, ohne
  Injection.
- **REQ-DIAG-026**: Die Latenz-Messreihe soll eine Markdown-Tabelle mit p50 und
  p95 für Loslassen→Text, STT, Cleanup und →erstes Segment sowie die WER je
  Fixture ausgeben, die Kaltlade-Dauer je Modell getrennt ausweisen und
  Commit-SHA, Datum und Gerät nennen.
- **REQ-DIAG-027**: WO die Variable `LATENCY_OUT` gesetzt ist, soll die
  Latenz-Messreihe die Tabelle zusätzlich in diese Datei schreiben.
- **REQ-DIAG-028**: FALLS ein Modell für die Latenz-Messreihe fehlt, DANN soll
  sie mit Pfad-Hinweis und Exit-Code ungleich 0 enden, ohne Panic.
- **REQ-DIAG-029**: FALLS eine Fixture nicht lesbar ist, DANN soll die
  Latenz-Messreihe sie überspringen und in der Tabelle als „übersprungen:
  <Grund>" ausweisen.
- **REQ-DIAG-035**: WO die Variable `LANGUAGE` (`auto`, `de`, `en`) gesetzt
  ist, soll die Latenz-Messreihe mit dieser Sprache laufen, sonst mit `auto`,
  und die verwendete Sprache in der Tabelle nennen; FALLS der Wert ungültig ist,
  DANN soll sie mit Hinweis und Exit-Code ungleich 0 enden. (Ticket-0039)
- **REQ-DIAG-034**: Die Latenz-Messreihe soll das Cleanup-Modell höchstens
  einmal pro Messreihe laden und dieselbe Instanz für alle Natürlich-Fälle
  verwenden, auch wenn ein Roh-Fall vorausging.

## Fixtures

- **REQ-DIAG-030**: WENN `scripts/fetch_fleurs_fixtures.sh` läuft, soll es
  eine fest per ID gewählte Liste von rund 15 deutschen FLEURS-Clips, davon
  mindestens 5 von Sprecherinnen, als
  `tests/fixtures/stt/fleurs-de-*.wav` (16 kHz mono) mit `.ref.txt` ablegen,
  bei jedem Lauf identisch.
- **REQ-DIAG-031**: Das Repo soll für die FLEURS-Fixtures Urheber, Titel,
  Quelle, Lizenz (CC-BY-4.0, mit Link) und vorgenommene Änderungen in
  `tests/fixtures/stt/ATTRIBUTION.md` nennen und in `docs/licensing.md`
  ausweisen, dass diese Dateien nicht unter der Code-Lizenz stehen.

## Regression „erstes Wort"

- **REQ-DIAG-032**: WENN die Audioquelle eine Fixture vollständig ab dem
  ersten Block liefert, soll das System das erste Wort der Referenz im
  Ergebnistext liefern, im Einfüge-Verhalten Am Ende und Satzweise und mit
  Sprache `de` und `auto` (die Pipeline verliert nichts, was die Quelle
  liefert; Startverzögerungen prüft REQ-DIAG-024). Ausnahmen gelten nur für
  einzeln benannte Fixtures mit Fehlerkennung des ersten Worts bei `auto`, mit
  Messbeleg und Zustimmung des Menschen im Ticket.
- **REQ-DIAG-033**: WO Satzweise aktiv ist und zwei Sätze mit kurzer Pause
  gesprochen werden, soll das System das erste Wort des zweiten Satzes im
  Ergebnistext liefern.
