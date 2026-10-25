# Streaming-STT und Voxtral für talker

Stand: 2026-10-25. Recherche gegen Primärquellen (Hersteller-Docs, Model Cards, GitHub-Repos der Eigentümer). Alles ohne Quellenangabe oder mit „nicht verifiziert" markiert ist nicht belegt. Hinweis: Die Seiten wurden über ein Abruf-Tool mit Zusammenfassung gelesen, nicht im Volltext; Zahlen vor Übernahme in ADRs bitte am Original gegenprüfen.

## Fragestellung

Ist Live-/Streaming-Diktat in talker machbar (heute: Parakeet TDT 0.6b v3 int8 via `sherpa-rs` 0.6.8 `TransducerRecognizer`, Offline/Batch, Text erst nach Loslassen der PTT-Taste)?

1. Welche Online-Modelle bietet sherpa-onnx mit Deutsch?
2. Unterstützt `sherpa-rs` (Cargo.lock: 0.6.8) die Streaming-API?
3. Voxtral als weitere `Transcriber`-Impl?
4. Design-Implikationen.

## Kurzfazit (Empfehlung)

- **Machbar, ja.** Das beste belegte Modell ist **NVIDIA Nemotron 3.5 ASR Streaming 0.6B** (multilingual inkl. Deutsch, cache-aware FastConformer-RNN-T, Chunks 80 ms bis 1,12 s, Interpunktion/Groß-/Kleinschreibung eingebaut). sherpa-onnx liefert dafür ein fertiges int8-Paket (560-ms-Chunk) und unterstützt es seit v1.13.x.
- **Wichtigste Hürde ist der Crate, nicht das Modell:** `sherpa-rs` ist **archiviert und deprecated**; die Modulliste von 0.6.8 enthält keinen Online-Recognizer. Weg: auf den offiziellen Crate **`sherpa-onnx`** (aktuell 2.0.0, hat `OnlineRecognizer`/`OnlineStream`) wechseln. Das ist ohnehin nötig, weil `sherpa-rs` keine Updates mehr bekommt (inkl. neuer Modelle wie Nemotron 3.5).
- **Voxtral Realtime (4B)** ist qualitativ attraktiv (Apache 2.0, 13 Sprachen inkl. Deutsch, DE-WER 6,19 % bei 480 ms), lokal auf Apple Silicon aber nur über Python/MLX (`mlx-audio`) sinnvoll nutzbar; **llama.cpp unterstützt Voxtral Realtime nicht** (PR/Issue geschlossen). Für eine Rust-Single-Binary-App aktuell keine gute `Transcriber`-Impl; als spätere Option beobachten.
- **Empfohlene Reihenfolge:** (1) Migration `sherpa-rs` → `sherpa-onnx` 2.x mit unverändertem Batch-Verhalten (Parakeet über `OfflineRecognizer`); (2) PoC Nemotron 3.5 Streaming mit Overlay-Vorschau; (3) Live-Einfügen in Ziel-App erst nach Field-Test, da Partials revidiert werden.
- Ungeklärt/Messung nötig: tatsächliche Latenz, RAM und DE-Qualität von Nemotron 3.5 int8 auf Apple Silicon (keine Primärquelle gefunden).

## Befunde

### Frage 1: Streaming-Modelle in sherpa-onnx mit Deutsch

**Offizielle Online-Transducer-Liste (Zipformer/Conformer/LSTM):** enthält Bengalisch, Chinesisch, Koreanisch, Englisch, Französisch, Chinesisch+Englisch; **kein Deutsch, keine NeMo-Modelle.**
Quelle: https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/index.html

**Nemotron-Streaming (NeMo) in sherpa-onnx:** Die NeMo-Doku listet zwei Pakete:
- `sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25` (Englisch)
- `sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11` (multilingual; Sprachliste steht dort nicht, siehe Model Card unten)
Dateigrößen nennt die Seite nicht.
Quelle: https://k2-fsa.github.io/sherpa/onnx/nemo/index.html

**Unterstützungshistorie:** Issue #3664 „Support multilingual Nemotron-3.5 streaming ASR" wurde am 2026-06-12 via PR #3671 geschlossen. Das Modell hat einen zusätzlichen Encoder-Input `prompt_index`; die Sprache wird per Stream als String gesetzt (`en`, `fr`, leer = Auto). Release v1.13.5 nennt drei Nemotron-3.5-Einträge (ONNX-Re-Export, Export-Skript, QNN-Export); v1.13.7 „per-stream runtime options for streaming ASR"; v2.0.0 (23. Oktober) „modified_beam_search with hotwords for streaming NeMo transducer models".
Quellen: https://github.com/k2-fsa/sherpa-onnx/issues/3664, https://github.com/k2-fsa/sherpa-onnx/releases

**Nemotron 3.5 ASR Streaming 0.6B (Model Card, NVIDIA):**
- 600M Parameter, 24-Layer cache-aware FastConformer-Encoder, RNN-T-Decoder; Lizenz OpenMDW-1.1 („ready for commercial use").
- Deutsch (de-DE) in der Tier „Transcription-ready".
- Chunk-Größen 80 ms / 160 ms / 320 ms / 560 ms / 1,12 s; kleinere Chunks = geringere Latenz, größere = bessere Genauigkeit.
- Deutsche WER auf FLEURS (LangID gesetzt): 9,81 % (80 ms), 9,21 % (160 ms), 8,83 % (320 ms), 8,42 % (560 ms), 8,31 % (1,12 s). Auto-Detect: 9,90 / 9,37 / 8,87 / 8,58 / 8,22 %.
- Interpunktion und Groß-/Kleinschreibung eingebaut; im Auto-Modus wird ein Sprach-Tag (`<en-US>`) angehängt.
- Getestet auf H100, Linux, NeMo 26.06; Apple-Silicon-Zahlen gibt es auf der Karte nicht.
Quelle: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b

**Vergleich zu Parakeet TDT 0.6b v3 (heutiges Modell):** 25 europäische Sprachen, CC BY 4.0, DE-WER FLEURS 5,04 %, CoVoST 4,84 % (ohne Interpunktion/Groß-/Kleinschreibung gemessen). Die Nemotron-Karte vergleicht nicht mit Parakeet v3 und die Messbedingungen sind nicht zwingend identisch (nicht verifiziert); als grobe Richtung ist Nemotron-Streaming für Deutsch ca. 3–5 Punkte WER schlechter als Parakeet-Batch, ein sauberer Vergleich fehlt. Eine Streaming-Variante von Parakeet TDT v3 selbst gibt es nicht; die Karte beschreibt nur NeMo-Chunked-Inference mit Beispiel 2-s-Chunks und nennt keine Streaming-WER.
Quelle: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3

**Kroko (Banafo) Streaming-Zipformer Deutsch:** Paket `csukuangfj/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06` existiert. Die Karte nennt weder Lizenz (Verweis auf `Banafo/Kroko-ASR`), Größe noch WER. Lizenz getrennt prüfen; Drittquellen sprechen von Dual-Lizenz (nicht verifiziert).
Quelle: https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06

**Parakeet Realtime EOU 120M:** nur Englisch, 80–160 ms Latenz, EOU-Token, keine Interpunktion; für talker (Deutsch) ungeeignet.
Quelle: https://huggingface.co/nvidia/parakeet_realtime_eou_120m-v1

**Paraformer:** In den abgerufenen Quellen kein deutsches Streaming-Paraformer gefunden (nicht verifiziert, dass keines existiert).

**Alternative ohne echtes Streaming:** sherpa-onnx-Rust-Beispiele „simulated streaming" (Silero-VAD + Offline-Recognizer) existieren. Damit ließe sich Parakeet v3 pseudo-live pro VAD-Segment nutzen, ohne Partials.
Quelle: https://github.com/k2-fsa/sherpa-onnx/tree/master/rust-api-examples

### Frage 2: Streaming in sherpa-rs / Weg

- Repo `thewh1teagle/sherpa-rs` ist **archiviert (read-only)**; README: „This crate is deprecated", Empfehlung: offizielle sherpa-onnx-Rust-API. Quelle: https://github.com/thewh1teagle/sherpa-rs
- Modulliste von `sherpa-rs` 0.6.8 (= Version im Cargo.lock): audio_tag, diarize, dolphin, embedding_manager, keyword_spot, language_id, moonshine, paraformer, punctuate, sense_voice, silero_vad, speaker_id, ten_vad, transducer, tts, whisper, zipformer. **Kein `online`-Modul.** `transducer::TransducerRecognizer` wird in talker für Offline-Decoding genutzt. Quelle: https://docs.rs/sherpa-rs/0.6.8/sherpa_rs/ . Nicht im Quellcode gegengeprüft, ob `sherpa-rs-sys` die Online-C-Funktionen als Roh-Bindings enthält.
- **Offizieller Crate `sherpa-onnx` 2.0.0** (Apache-2.0, safe Wrapper, standardmäßig statisch gelinkt): `OnlineRecognizer`, `OnlineStream`, `OnlineRecognizerConfig`, `OnlineTransducerModelConfig`, `OfflineRecognizer` etc. Es gibt **keinen Nemotron-spezifischen Config-Typ** in der Struct-Liste; wie Nemotron-Streaming-Modelle in der Rust-API konfiguriert werden (vermutlich über `OnlineTransducerModelConfig`), ist nicht belegt und im PoC zu klären. Quelle: https://docs.rs/sherpa-onnx/latest/sherpa_onnx/
- Rust-Beispiele für Streaming: `streaming_zipformer_en`, `streaming_zipformer_zh_en`, `streaming_zipformer_microphone`; kein Nemotron-Beispiel gelistet. Quelle: https://github.com/k2-fsa/sherpa-onnx/tree/master/rust-api-examples
- Der Crate wird aktiv gepflegt (Rust-Fixes in v1.13.7, v1.13.8, v2.0.0). Quelle: https://github.com/k2-fsa/sherpa-onnx/releases
- Fazit: Migration auf `sherpa-onnx` ist der Weg; sie berührt `Cargo.toml` und `src/stt.rs` (`ParakeetTranscriber`). Das Migrationsrisiko (Build/Download-Binaries, macOS-Linking) wurde nicht geprüft.

### Frage 3: Voxtral (Mistral)

**Varianten (alle Apache-2.0):**

| Modell | Größe | Sprachen | Notiz | Quelle |
|---|---|---|---|---|
| Voxtral Mini 3B (2507) | Name 3B, Safetensors-Metadaten 5B (F32/BF16) | 8: en, es, fr, pt, hi, de, nl, it | ~9,5 GB GPU-RAM bf16; Kontext 32k; Audio bis 30 min | https://huggingface.co/mistralai/Voxtral-Mini-3B-2507 |
| Voxtral Small (2507) | 24B | dieselben 8 | ~55 GB GPU-RAM bf16 | https://huggingface.co/mistralai/Voxtral-Small-24B-2507 |
| Voxtral Mini 4B Realtime (2602) | ~4B (≈3,4B LM + ≈970M Encoder laut Karte; die mlx-community-Karte nennt ≈0,6B Encoder, Widerspruch) | 13: ar, de, en, es, fr, hi, it, nl, pt, zh, ja, ko, ru | natives Streaming, Delay 80 ms bis 2,4 s (Empfehlung 480 ms); ≥16 GB GPU | https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602 |
| Voxtral Mini Transcribe V2 | nicht angegeben | 13 | nur API (Batch), ~4 % WER FLEURS (Top-10-Sprachen), Gewichte laut Artikel nicht offen (nicht verifiziert) | https://mistral.ai/news/voxtral-transcribe-2 |

**Benchmarks (belegt):**
- Realtime, Deutsch, FLEURS: 6,19 % WER bei 480 ms Delay vs. 3,54 % für Voxtral Mini Transcribe 2.0 (offline). Karte: https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602
- Mistral: bei 2,4 s Delay gleichauf mit Mini V2, bei 480 ms innerhalb 1–2 % WER (herstellerseitig). Das Paper sagt „auf Augenhöhe mit Whisper" bei 480 ms, ohne Zahlen in der abgerufenen Zusammenfassung. Quellen: https://mistral.ai/news/voxtral-transcribe-2, https://arxiv.org/abs/2602.11298
- Voxtral Mini/Small 2507: Whisper-Vergleiche nur als Bild auf den Karten; **kein Text-Wert für Deutsch**. Nicht verifiziert.
- Vergleich mit Parakeet v3 in Primärquellen **nicht vorhanden**. Als Einordnung: Parakeet v3 DE-FLEURS 5,04 % (Batch) vs. Voxtral Realtime 6,19 % (480 ms) – andere Normalisierung möglich, daher nur grob vergleichbar (nicht verifiziert).

**Lokale Inferenz auf Apple Silicon:**
- **MLX / `mlx-audio`** (MIT, Python): Voxtral Realtime mit 4-bit und fp16, Streaming-Ausgabe (`stream=True`). 4-bit-Gewichte: 3,13 GB. Laufzeit-RAM und Latenz auf Mac nicht angegeben (nicht verifiziert). Quellen: https://github.com/Blaizzy/mlx-audio, https://huggingface.co/mlx-community/Voxtral-Mini-4B-Realtime-2602-4bit
- **llama.cpp:** Voxtral (3B/24B, Offline-Audio) seit Juli 2025 über `mtmd` (PR #14862, nur aus Suchergebnis belegt). **Voxtral Realtime nicht unterstützt:** PR #20638 am 2026-03-23 ohne Merge geschlossen (modellspezifische Pfade in libmtmd), Planungs-Issue #20914 zuletzt am 2026-07-17 als „not planned" geschlossen. Quellen: https://github.com/ggml-org/llama.cpp/pull/20638, https://github.com/ggml-org/llama.cpp/issues/20914
- Weitere Implementierungen laut Karte: vLLM (empfohlen, Realtime-Endpoint, GPU), Transformers ≥ 5.2.0, Community: voxtral.c, mlx-audio, Rust-Implementierung (nicht geprüft), audio.cpp (laut Kommentar im Issue). Nicht verifiziert, ob eine davon produktionsreif auf macOS in Rust nutzbar ist.
- Latenz/RAM auf Apple Silicon: **keine Primärquelle**. Mess-PoC nötig.

**Bewertung als `Transcriber`-Impl:**
- Pro: Apache 2.0, 13 Sprachen, echtes Streaming mit einstellbarem Delay, Modellqualität laut Hersteller nahe Offline.
- Contra: ~4B Parameter (3,13 GB selbst in 4-bit) gegenüber ~0,6B bei Parakeet/Nemotron; zusätzlich läuft schon Gemma über llama.cpp (ADR-0003), sodass RAM/Startzeit/Energie kumulieren; kein Rust-/C-Pfad mit offizieller Unterstützung (llama.cpp nein, sherpa-onnx in den abgerufenen Quellen nicht erwähnt); Python/MLX-Sidecar widerspricht dem „Rust, Single-Binary, FFI hinter Traits"-Ansatz (ADR-0001), ist aber technisch hinter dem Trait kapselbar.
- **Empfehlung:** Nicht jetzt umsetzen. Nemotron 3.5 über sherpa-onnx deckt Streaming mit Deutsch bei ~0,6B ab. Voxtral Realtime neu bewerten, falls (a) llama.cpp/ein C-ABI-Runtime es unterstützt oder (b) ein Mess-PoC über mlx-audio deutlich bessere deutsche Diktatqualität als Nemotron zeigt.

### Frage 4: Design-Implikationen für talker

Dies sind Designvorschläge, keine Fakten aus Quellen.

**Trait-Erweiterung:** Der bestehende `Transcriber::transcribe(&mut self, &[f32]) -> Result<String>` bleibt für Batch. Für Streaming ein separater Trait (kein Mischen, YAGNI), etwa:
- `StreamingTranscriber { fn start(&mut self) -> Result<()>; fn accept(&mut self, pcm_16k_mono: &[f32]) -> Result<Partial>; fn finish(&mut self) -> Result<String>; }`
- `Partial` unterscheidet stabil (committed) von instabil (Hypothese). sherpa-onnx liefert pro Stream einen aktuellen Gesamttext und Endpoint-Erkennung (Online-API: `OnlineStream` + `OnlineRecognizer`); das Modell revidiert den Text, die Differenz muss der Aufrufer bilden.
- Batch-Fallback (Parakeet) bleibt als `Transcriber`; die Pipeline wählt per Config (neues Feld mit `#[serde(default)]`, laut docs/stability.md).

**Overlay-Vorschau vs. Live-Einfügen:**
- **Overlay-Vorschau zuerst:** Partials im eigenen Overlay anzeigen, beim Loslassen den finalen Text wie heute einfügen. Revisionen sind kostenlos, Injection-/Clipboard-Logik bleibt unverändert. Geringes Risiko.
- **Live-Einfügen in die Ziel-App:** Partials werden revidiert (Zeichen müssen per Backspace-Events zurückgenommen werden), Apps mit Autokorrektur, Undo-Stack oder Rich-Text verhalten sich unterschiedlich; Cursor-/Fokuswechsel während der Aufnahme sind riskant. Wenn überhaupt, nur stabile Anteile einfügen (z. B. nach Endpoint oder fester Verzögerung) – das ist ein Designvorschlag, nicht belegt.

**Zusammenspiel mit Cleanup (Gemma/llama.cpp):** Cleanup ist satzweise sinnvoll, nicht auf Partials. Vorschlag: Rohtext live im Overlay, Cleanup erst nach PTT-Release (bzw. pro Endpoint-Segment) auf dem finalen Raw Transcript, Fallback auf Raw bei Fehler bleibt (Projektregel). Live-Einfügen und Cleanup schließen sich weitgehend aus, weil Cleanup bereits eingefügten Text verändern würde. Nemotron liefert bereits Interpunktion/Groß-/Kleinschreibung; ob das den Cleanup-Aufwand verringert, ist zu messen.

**Weitere Punkte:** Pipeline darf nicht hart abbrechen (Fallback auf Batch-Transkription des gepufferten Audios bei Streaming-Fehler); Tests pro AK inkl. Grenzfälle (leere Aufnahme, sehr kurze Aufnahme unter Chunk-Größe, Endpoint mitten im Satz).

## Offene / nicht verifizierte Punkte

- Latenz, RAM und CPU/ANE-Last von Nemotron 3.5 int8 (560-ms-Paket) auf Apple Silicon: keine Primärquelle.
- Dateigröße des sherpa-onnx-Nemotron-3.5-int8-Pakets (HF-Seite war nicht abrufbar, HTTP 401). OpenWhispr nennt ~650 MB (Drittquelle, Blogpost: https://openwhispr.com/blog/local-streaming-speech-to-text) – nicht verifiziert.
- Direkter, gleich normalisierter DE-WER-Vergleich Parakeet v3 vs. Nemotron 3.5 vs. Voxtral Realtime: nicht vorhanden; eigener Eval im Repo (`docs/eval`) nötig.
- Konfiguration von Nemotron 3.5 in der Rust-API `sherpa-onnx` 2.0.0 (kein eigener Config-Typ gefunden) und Sprachwahl pro Stream aus Rust.
- Ob `sherpa-rs-sys` 0.6.8 Online-C-Funktionen als Roh-Bindings exponiert (nicht im Quellcode geprüft).
- Lizenz der Kroko-Gewichte (Karte verweist auf Banafo).
- Voxtral: Encoder-Größe (970M vs. 0,6B), Whisper-Vergleichswerte Deutsch, Runtime-RAM auf Mac, Reife der Rust-/audio.cpp-Implementierungen.
- Migrationsaufwand `sherpa-rs` → `sherpa-onnx` (Binary-Download, Linking unter macOS, Feature-Parität für `TransducerRecognizer`).
- Paraformer-Streaming für Deutsch: nichts gefunden, Nichtexistenz nicht belegt.
