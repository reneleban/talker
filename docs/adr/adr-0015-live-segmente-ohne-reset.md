# ADR-0015 — Live-Segmente ohne Recognizer-Reset

- Status: akzeptiert
- Datum: 2026-11-08
- Kontext: Ticket-0039 (Bug D), `docs/requirements/streaming-dictation.md` (REQ-STREAM-016),
  `docs/requirements/diagnostics.md` (REQ-DIAG-033)
- Baut auf: ADR-0009 (Streaming-Stufen), ADR-0001 (Engine hinter Trait)

## Kontext und Problemstellung

Im Live-Modus (Satzweise, Wortgruppenweise) beendet eine Sprechpause ein Segment.
Bisher setzte `NemotronTranscriber::feed` bei `is_endpoint` den Recognizer per
`reset()` zurück und speiste danach 0,4 s Nullen ein. Zwei Fehler folgen daraus:

1. **Stille mitten im Audio.** `reset()` verwirft kein Audio. Noch nicht dekodierte
   Frames (bis zu einem Chunk samt Lookahead) bleiben im Stream und kommen nach
   dem Reset zuerst dran. Die 0,4 s Nullen landen dahinter, also mitten im
   nächsten Satz, wenn dieser schon im offenen Rest beginnt.
2. **Kalter Neustart.** `reset()` setzt Encoder- und Decoder-Zustand auf den
   Anfangswert. Ohne die Nullen (Variante D1, in Ticket-0039 gemessen) ist der
   Modell-Eingang lückenlos, aber der Satzanfang nach der Pause fällt weg
   (»Dies ist« fehlt im bisher grünen Test `streaming_pause_ends_the_segment_with_an_endpoint`).

Fakten (Primärquellen, sherpa-onnx `v2.0.0`, das Tag, dessen Bibliotheken
`sherpa-onnx-sys` 2.0.0 lädt):

- `online-recognizer-transducer-nemo-impl.h`, `Reset`: `SetResult({})`,
  `SetStates(GetEncoderInitStates())`, `SetNeMoDecoderStates(GetDecoderInitStates())`;
  Kommentar „We only update counters. The underlying audio samples are not discarded.“
- `online-stream.cc`, `Reset`: `start_frame_index_ += num_processed_frames_; num_processed_frames_ = 0`.
- `online-transducer-greedy-search-nemo-decoder.cc`: Ein Nicht-Blank-Token wird
  nur per `r.tokens.push_back(y)` angehängt und setzt `num_trailing_blanks = 0`;
  ein Blank zählt `num_trailing_blanks` hoch. Frühere Tokens werden nie geändert.
- `online-recognizer-transducer-impl.h`, `Convert`: `text` entsteht durch Anhängen
  der Token-Symbole in Reihenfolge. Inverse Textnormalisierung und
  Homophon-Ersetzung laufen nur, wenn konfiguriert; talker konfiguriert beides nicht.
  `tokens.txt` des Modells enthält keine Byte-Fallback-Symbole (`<0x..>`).
- `endpoint.h`/`endpoint.cc`: `rule1` hat `must_contain_nonsilence = false`, feuert
  also auch ohne erkannten Text. Die Länge zählt `num_processed_frames` seit dem
  letzten Reset, die Stille `num_trailing_blanks` (also Blank-Frames, nicht Pegel).
  Die C-API (`c-api.cc`) setzt nur die drei Schwellen, nicht `must_contain_nonsilence`.

## Entscheidung

**Eine Live-Utterance nutzt einen einzigen sherpa-Stream ohne `reset()`.
Segment-Grenzen bestimmt der Adapter aus dem Textzuwachs.**

- `NemotronTranscriber` merkt sich den Text bis zur letzten Segment-Grenze
  (`committed`) und die seitdem eingespeisten Samples.
- Text des offenen Segments = Gesamttext ohne das Präfix `committed`, getrimmt.
- Segment-Ende, wenn das offene Segment Text hat und
  `is_endpoint` wahr ist **oder** seit der Grenze ≥ 60 s Audio eingespeist wurden
  (Zwangs-Endpoint, bisher `rule3`). Dann rückt die Grenze auf den Gesamttext vor.
- `rule3_min_utterance_length` wird so groß gesetzt, dass `rule3` nie feuert;
  `rule3` zählt ohne Reset ab dem Stream-Start und wäre nach 60 s dauerhaft wahr.
- `is_endpoint` ohne neuen Text ist kein Segment-Ende und hat keine Wirkung.
- Vor- und Nachlauf bleiben nur am Anfang (`begin`) und Ende (`finish`) der Utterance.
- Die Schnittstelle bleibt: `StreamingTranscriber::{begin, feed, finish}` und
  `StreamUpdate { text, endpoint }` mit derselben Bedeutung für die Pipeline.

## Beleg (Wegwerf-Experiment, Ticket-0039, 2026-11-08)

D2 als Prototyp in `stt.rs`, Nemotron lokal, `language = auto`:

| Test | vorher (Reset + 0,4 s Nullen) | D1 | D2 |
|---|---|---|---|
| `streaming_pause_ends_the_segment_with_an_endpoint` | grün | rot (»Dies ist« fehlt) | grün |
| Tap lückenlos (AK-11, 3 Tests) | rot | grün | grün |
| AK-28 Satzpaare rot | 3 von 3 | 1 von 3 | 1 von 3 (siehe Ticket, Testannahme) |
| Satzweise FLEURS, Fehlfälle | 8 | 8 | 2 (= Am Ende) |
| Satzweise say, Fehlfälle | 3 | 3 | 1 (= Am Ende) |

Mit D2 hat Satzweise dieselben Fehlfälle wie der Batch-Pfad (fleurs-de-03, -13,
dev-java). Die übrigen 16 Modelltests blieben grün; ein Präfixfehler trat nicht auf.

## Betrachtete Optionen

1. **D1 — `reset()` ohne Vorlauf.** Lückenlos, kleinste Änderung. Der kalte
   Neustart kostet den Satzanfang nach der Pause (gemessen). Verworfen.
2. **D2 — ein Stream ohne Reset, Segmente aus dem Textzuwachs (gewählt).**
   Lückenlos, ohne Stille, Encoder und Decoder bleiben warm. Leere Endpoints
   (`rule1` ohne Text, z. B. direkt nach dem Vorlauf) richten keinen Schaden mehr an.
   Kosten: Zustand im Adapter, Zwangs-Endpoint in talker statt in sherpa.
3. **D3 — neuer Stream und erneutes Einspeisen des offenen Rests.** Der Stand
   `num_processed_frames` ist über die C-API nicht lesbar, der offene Rest also
   nur durch Nachbau sherpa-interner Rechnungen bestimmbar. Doppelte oder
   fehlende Wörter drohen. Verworfen.
4. **D4 — `reset()` und echtes Audio der letzten Zeit erneut einspeisen.**
   Das Modell sähe Audio doppelt (REQ-STREAM-016 verletzt, doppelte Wörter). Verworfen.
5. **D1 mit Reset nur bei Text.** Vermeidet leere Resets, der kalte Neustart nach
   einem echten Segment bleibt. Verworfen.

## Konsequenzen

- Der Modell-Eingang einer Live-Utterance ist genau `Vorlauf ‖ Eingang ‖ Nachlauf`
  (REQ-STREAM-016, prüfbar über `ModelTap`).
- Speicher: Das Audio einer Utterance bleibt bis `finish` im Stream. Das galt schon
  mit `reset()` (sherpa verwirft keine Samples), ist also nicht neu.
- Der Gesamttext wächst mit der Utterance; `get_result` je `feed` kopiert ihn.
  Bei einer gedrückt gehaltenen Taste ist das vernachlässigbar.
- Satzzeichen, die das Modell erst nach der Pause ausgibt, stehen am Anfang des
  nächsten Segments oder bilden ein eigenes Segment (im Experiment: `" ."`,
  `" , dass er …"`). Der Adapter reicht sie unverändert weiter. Die Pipeline
  (`DictationWorker::live_segment`) fügt ein Segment, das mit einem Satzzeichen
  beginnt, ohne führendes Leerzeichen ein; das Satzzeichen geht nicht durch den
  Cleanup.
- Präfixstabilität ist für Greedy-Search belegt (siehe Fakten). Wechselt talker die
  Dekodierung (z. B. `modified_beam_search`) oder aktiviert ITN bzw.
  Homophon-Ersetzung, ist diese Entscheidung neu zu prüfen. Ist `committed` wider
  Erwarten kein Präfix, nimmt der Adapter den Text ab dem gemeinsamen Präfix und
  loggt eine Warnung, ohne Panic. Dabei kann ein Wort doppelt erscheinen, verloren
  geht keines.
- `StreamUpdate`-Doku („Recognizer bereits zurückgesetzt“) entfällt.
