# ADR-0009 — Live-Diktat in zwei Stufen: Einfüge-Verhalten und Korrektur

- Status: akzeptiert
- Datum: 2026-10-25
- Kontext: siehe `CONTEXT.md`, `docs/research/streaming-stt-und-voxtral.md`
- Baut auf: ADR-0001 (Engines hinter Traits), ADR-0008 (Nemotron als einziges STT-Modell)

## Kontext und Problemstellung

Heute erscheint der Text erst nach dem Loslassen der PTT-Taste („Am Ende").
Ziele des Live-Diktats: (a) Feedback schon beim Sprechen, (b) weniger gefühlte
Wartezeit, (c) Text live im Zielfeld. Offen war, wie Live-Einfügen mit den
Cleanup-Modi (Roh, Geschäftlich, Natürlich, LLM-optimiert) zusammenspielt,
denn bereits eingefügter Text lässt sich in fremden Apps nicht zurücknehmen.

## Entscheidung

Zwei unabhängig einstellbare Stufen:

1. **Einfüge-Verhalten** (`insert_mode`): *Am Ende* (heutiges Verhalten, Default),
   *Satzweise* (Einfügen bei Sprechpause, Länge `endpoint_silence_ms`) oder
   *Wortgruppenweise* (Einfügen, sobald Wörter stabil sind).
2. **Korrektur** (bestehender `cleanup_mode`), erlaubt je nach Stufe 1:

| Einfüge-Verhalten | Erlaubte Modi |
|---|---|
| Am Ende | Roh, Geschäftlich, Natürlich, LLM-optimiert |
| Satzweise | Roh, Geschäftlich, Natürlich |
| Wortgruppenweise | Roh |

Begründung der Matrix: LLM-optimiert braucht den Gesamttext. Wortgruppenweise
liefert zu wenig Kontext für Korrektur, und einmal eingefügter Text wird nie
zurückgenommen.

Technik:

- **Audio:** Der Audio-Callback bekommt einen zweiten Verbraucher, der Chunks
  über einen Kanal an einen Streaming-Worker-Thread mit dem Nemotron-
  `OnlineRecognizer` gibt. Bei *Am Ende* existiert dieser Verbraucher nicht;
  der Batch-Pfad bleibt unverändert.
- **Laden:** Beim App-Start, und nur, was konfiguriert ist.
- **Fallback:** Ist Live nicht verfügbar (Modell fehlt, Worker-Fehler), fällt
  talker auf *Am Ende* zurück; ein Hinweis erscheint nur in den Settings.
- **Ein Modell:** Batch und Streaming nutzen dasselbe Nemotron-Modell.
- **Kontextregeln:** Liefert eine App-Regel (`context_rules`) einen in Stufe 1
  nicht erlaubten Modus, gilt der nächste erlaubte (Roh bei Wortgruppenweise).
- **Settings-UI:** Zwei Auswahlfelder; nicht erlaubte Modi sind ausgegraut mit
  Hinweis. Wechselt Stufe 1 auf einen Wert, der den gewählten Modus nicht
  erlaubt, springt Stufe 2 automatisch auf den nächsten erlaubten Modus.
  `endpoint_silence_ms` ist nur in der Config-TOML einstellbar (YAGNI).

## Betrachtete Optionen

1. **Overlay-Vorschau, Einfügen am Ende:** verworfen — Ziel (c) Text live im Zielfeld.
2. **Eine Stufe mit gekoppelten Modi:** verworfen — Cleanup-Modi und Einfüge-Zeitpunkt sind unabhängige Wünsche.
3. **Zwei Stufen mit Erlaubnismatrix:** gewählt.

## Konsequenzen

- Neue Config-Felder `insert_mode` (Default *Am Ende*) und `endpoint_silence_ms`
  mit `#[serde(default)]`: kein Breaking Change, Bestandsconfigs verhalten sich wie vorher.
- Ein bereits eingefügter Satz bzw. eine Wortgruppe bleibt stehen, auch wenn der
  Recognizer sie später anders erkennen würde.
- Live-Modi halten zusätzlich zum Recognizer einen Worker-Thread; RAM bleibt
  bei einem Modell (ca. 1,2 GB Peak laut ADR-0008-Messung).
