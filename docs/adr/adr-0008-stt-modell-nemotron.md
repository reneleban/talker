# ADR-0008 — STT-Modell: Nemotron 3.5 Streaming ersetzt Parakeet

- Status: akzeptiert
- Datum: 2026-10-25
- Kontext: siehe `CONTEXT.md`, `docs/research/streaming-stt-und-voxtral.md`
- Baut auf: ADR-0001 (Engines hinter Traits austauschbar)

## Kontext und Problemstellung

talker nutzte Parakeet TDT 0.6b v3 im Batch-Betrieb (Text erst nach dem Loslassen).
Geplant ist Live-Diktat (satzweises bzw. wortgruppenweises Einfügen). Das setzt ein
Streaming-Modell voraus; Parakeet TDT v3 gibt es nicht als Streaming-Variante. Zu
entscheiden war, ob talker zwei STT-Modelle betreibt (Parakeet für „Am Ende",
Nemotron für Live) oder eines für alle Modi.

## Entscheidungstreiber

- Eine Lösung statt mehrerer: ein Modell, ein Download, eine Lizenz, ein Codepfad.
- Deutsch (mit englischen Fachbegriffen) ist die Hauptsprache.
- Live-Diktat soll ohne Wechsel des Modells möglich sein.
- Wartezeit nach dem Loslassen und Erkennungsqualität im Alltag.

## Betrachtete Optionen

1. **Zwei Modelle:** Parakeet für „Am Ende", Nemotron nur für Live.
2. **Nemotron 3.5 ASR Streaming 0.6b für alles** (560-ms-Chunks, int8).
3. **Voxtral Realtime:** verworfen — kein Rust-/C-Pfad (llama.cpp ohne Support),
   siehe Research-Datei.

## Entscheidung

**Option 2:** Nemotron 3.5 Streaming ersetzt Parakeet als einziges STT-Modell.

## Messung (24 Fixtures aus `tests/fixtures/stt/`, `make stt-eval`; TTS, ein Sprecher — indikativ)

| | Parakeet | Nemotron |
|---|---|---|
| Wartezeit nach dem Loslassen (Mittel) | 0,35 s | ca. 0,7–1,0 s (8 Threads) |
| Mittlere WER | 15,0 % | 17,3 % |
| Begriff-Treffer | 19/29 | 18/29 |

## Begründung

- Nur ein Modell erfüllt beide Ziele (Batch heute, Live-Streaming danach) ohne
  Doppelbetrieb: ein Download (ca. 475 MB), ein Lizenz-Consent, ein Transcriber.
- Nemotron liefert Interpunktion und Groß-/Kleinschreibung bereits im Modell.
- Der Qualitätsunterschied liegt bei ca. 2 Punkten WER (roh) bzw. ca. 5 Punkten,
  wenn man die 4 Fälle ausklammert, in denen Parakeet Ziffern statt Zahlwörter
  schreibt (ohne diese 4 Fälle: Parakeet 14,0 %, Nemotron 19,2 %).

## Konsequenzen

- **Negativ (bewusst akzeptiert):** Die Wartezeit nach dem Loslassen steigt im
  Mittel um ca. 0,4–0,7 s; die WER steigt um ca. 2–5 Punkte (nur mit TTS-Audio
  gemessen — Messung mit echter Stimme steht aus).
- **Bestandsnutzer:** Das Modell ist ein anderes Paket (Pflicht-Modell wechselt →
  nach `docs/stability.md` ein Breaking Change). Die App fragt die neue Lizenz
  (OpenMDW-1.1) erneut ab (`stt_license_consent`, Default `false`) und lädt
  danach das neue Modell. `stt_model_dir`, das auf das alte Parakeet-Verzeichnis
  zeigt, wird beim Laden auf den neuen Default umgebogen. Das alte Modell
  bleibt auf der Platte und kann manuell gelöscht werden.
- **Technik:** Das Audio wird mit 0,4 s Vorlauf- und 1,2 s Nachlauf-Stille
  eingespeist (sonst fehlen Satzanfang und letzte Wörter); Threads
  `min(Kerne, 8)`. Kürzere Nachlauf-Stille spart keine Zeit, schneidet aber das
  letzte Wort ab.
- **Verworfen:** Zwei Modelle parallel (Option 1) — doppelter RAM (ca. 2,5 GB
  geschätzt), zwei Downloads und Lizenzen, zwei Codepfade.
