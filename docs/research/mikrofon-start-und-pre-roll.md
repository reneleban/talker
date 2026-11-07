# Mikrofon-Start, Pre-Roll und Latenz — Recherche

- Stand: 2026-11-07
- Methode: vier parallele Web-Recherchen (andere Diktat-Apps, macOS-Audiotechnik,
  Modell/Pre-Roll, Test und Messung) plus Abgleich mit dem talker-Code auf `main` (`e3ca022`/`ec5389c`).
- Ziel im Repo: Diese Datei gehört als `docs/research/mikrofon-start-und-pre-roll.md`
  in den Branch des ersten Tickets dieses Themas (nicht direkt auf `main`).
- „Nicht verifiziert" heißt: plausibel, aber ohne Primärquelle oder eigene Messung.

## Kurzfazit

- Fast alle Diktat-Apps öffnen das Mikrofon **pro Tastendruck** und haben dasselbe Problem.
- Bewährte Muster: (1) Startpfad schnell machen (Gerät/Config cachen), (2) ab dem ersten
  Callback lückenlos puffern und „bereit" erst dann anzeigen, (3) Mikro nach Nutzung
  **opt-in** warm halten mit Ringpuffer (~500 ms).
- **Dauerhaft offenes Mikro ist nirgends Default** (oranger macOS-Indikator, Datenschutz).
- **Bluetooth/AirPods** (HFP-Umschaltung 0,5–2 s) löst niemand sauber; Empfehlung der
  Hersteller: eingebautes/kabelgebundenes Mikro.

## Befunde im talker-Code

| # | Befund | Stelle | Sicherheit |
|---|---|---|---|
| C | Log „Mikrofon-Start dauerte" misst nur bis `play()` zurückkehrt, nicht bis zum ersten Callback / ersten Signal | `src/pipeline.rs:622-626` | sicher |
| C' | Overlay schaltet auf Wellen (`start_recording`), sobald `play()` zurück ist — womöglich bevor Audio fließt; Nutzer spricht zu früh | `src/indicator.rs:90`, `on_ready` in `src/pipeline.rs` | sicher (Ablauf), Wirkung nicht gemessen |
| A | Pro Tastendruck: Geräte-Aufzählung (bei konfiguriertem Namen), `default_input_config`, `set_physical_format`, AUHAL-Init (`initialize`), `AudioOutputUnitStart` | `src/audio.rs:125-205`; cpal 0.18.1 `host/coreaudio/macos/device.rs:717-724, 795`, `mod.rs:311-327` | Kosten je Schritt nicht gemessen |
| D | Live-Modus: nach `is_endpoint` → `reset()` (Encoder-Cache + Decoder kalt), dann 0,4 s Nullen **hinter** bereits angenommenes, noch nicht dekodiertes Audio (bis 1 Chunk = 560 ms). Beginnt der nächste Satz darin, wird er zerschnitten und kalt dekodiert | `src/stt.rs:239-246` | Bug sicher, Wirkung nicht gemessen |
| E | Vorlauf/Nachlauf sind digitale Nullen; NeMo trainiert mit `dither=1e-5`, bei Inferenz aus → reine Nullen sind „unbekanntes" Signal | `src/stt.rs:28-31`; NeMo `audio_preprocessing.py`, `features.py` | Hypothese, A/B nötig |
| — | Batch-Pfad hat bereits 0,4 s Vorlauf / 1,2 s Nachlauf (aus PoC) | `src/stt.rs:46-51` | sicher |

**Mikrofon oder Modell?** Rohsignal pro Diktat als WAV mitschreiben + Zeitstempel. Wort fehlt
schon in der WAV → Mikrofon/Start (A/B/C). Wort in der WAV, fehlt im Text → Modell/Stream (D/E);
Gegenprobe: dieselbe WAV durch `transcribe()` und durch `begin`/`feed`.

## 1. Andere Diktat-Apps

| Projekt | Ansatz | Beleg |
|---|---|---|
| Handy (Rust, cpal) | pro Druck (Default); `always_on_microphone` opt-in; `lazy_stream_close` schließt erst nach 30 s Leerlauf | github.com/cjpais/Handy (`src-tauri/src/managers/audio.rs`, `settings.rs`) |
| Handy #1283 / PR #1582 | Regression 16–21 ms → ~500 ms Start durch Geräte-Aufzählung + Config-Probe; Fix: Device/Config cachen | https://github.com/cjpais/Handy/issues/1283, https://github.com/cjpais/Handy/pull/1582 |
| Handy #1879 | 0,5–3 s verloren (Windows); Lösung: Indikator grau bis wirklich aufgenommen wird | https://github.com/cjpais/Handy/issues/1879 |
| OpenWhispr PR #1493 | Mikro ab Key-Down, Puffer ab Key-Down nahtlos übernommen, Warmhalten opt-in Off/10 s/1 min/15 min (Default Off), Pre-Roll max. 2 s alt | https://github.com/OpenWhispr/openwhispr/pull/1493, https://github.com/OpenWhispr/openwhispr/issues/742 |
| VoiceInk (Swift, AUHAL) | pro Aufnahme; offene Issues 0,5–1 s, Workaround eingebautes Mikro | https://github.com/Beingpax/VoiceInk/issues/996, https://github.com/Beingpax/VoiceInk/issues/572 |
| macos-mic-keepwarm | Hintergrunddienst hält Mikro per `AVCaptureSession` offen | https://github.com/drewburchfield/macos-mic-keepwarm |
| Wispr Flow | Doku: kurz pausieren, kabelgebundenes/eingebautes Mikro | https://docs.wisprflow.ai/articles/3566082841-fix-missing-first-words-in-transcriptions |
| Superwhisper | keine Warmhalte-Option; Startton verzögert (~1 s → ~0,25 s ohne Ton, Nutzer-Messung) | https://superwhisper.userjot.com/board/p/reduce-1-sec-latency-between-toggle-key-press-and-recording-start |
| whisper-writer, nerd-dictation | pro Aufnahme neuer Stream/Prozess | Quellcode der Projekte |

- Handy-Maintainer zu Always-on als Default: „not the default as we are more concerned about
  users questioning why the mic is always active" (#1283).
- Warmhalten bei Bluetooth: Headset bleibt im HFP-Modus → schlechtere Wiedergabe (keepwarm-README);
  AirPods-Handoff-Seiteneffekte (Handy #646).
- Pre-Roll-Größen: 500 ms (OpenWhispr #742), 600 ms (Kommentar Handy #1283); Handy `VAD_PREFILL_MS` 450 ms (nur innerhalb der Aufnahme).

## 2. macOS-Audiotechnik

- AirPods: Mikro-Nutzung schaltet A2DP/AAC → HFP/SCO (8/16 kHz). Dauer 1–2 s laut
  `aside`-Release-Notes (https://github.com/cmwright/aside/releases/tag/v0.3.2), ~0,5 s laut VoiceInk #996. Einzelberichte.
- Eingebaut/USB: keine belastbaren Zahlen gefunden (nicht verifiziert).
- Erste Buffer = Nullen/Warm-up: Einzelberichte in Apple-Foren (https://developer.apple.com/forums/thread/836713,
  https://developer.apple.com/forums/thread/854419), nicht dokumentiert.
- cpal `pause()` = `AudioOutputUnitStop`, Unit bleibt initialisiert → vorgebauter Stream mit
  `play()`/`pause()` je Druck möglich. Ob der orange Punkt nach `pause()` erlischt: **nicht verifiziert, selbst testen**.
- Oranger Punkt = Mikro „in Verwendung" (https://support.apple.com/en-au/guide/mac-help/mchl50f94f8f/mac/15.0);
  bleibt nach Stopp ggf. ~20 s sichtbar (https://developer.apple.com/forums/thread/742272). Kein offizieller Weg, ihn auszublenden.
- Ideen ohne Messbeleg: `AVAudioApplication.setInputMuted` (macOS 14+, I/O läuft weiter, stumm);
  `murmur-core` setzt Default-Input neu, um SCO früh aufzubauen (https://docs.rs/murmur-core/0.1.3/src/murmur_core/audio/activate.rs.html).
- AVAudioEngine/AVCaptureSession/VPIO/Hog/Aggregate: kein Beleg für geringere Startlatenz.
- Messen: cpal `InputCallbackInfo.timestamp()` (`callback`, `capture`, aus `mHostTime`), `Stream::now()` gleiche Zeitbasis.

## 3. Modell und Pre-Roll

- Amazon AVS: 500 ms Pre-Roll Pflicht (Ringpuffer, ein durchgehender Strom).
  https://developer.amazon.com/docs/alexa-voice-service/implement-ww-verification.html (nur via Suchzusammenfassung)
- wyoming-satellite `--vad-buffer-seconds` Default 2 s; Silero-VAD `speech_pad_ms=30`.
- sherpa-onnx #4127 / PR #4133 (Parakeet-Unified, nicht unser Pfad): Nullen im linken Kontext und
  `Reset()` des Predictors verursachten verlorene Äußerungen (WER 52 % → 7,7 % nach Fix).
  https://github.com/k2-fsa/sherpa-onnx/issues/4127, https://github.com/k2-fsa/sherpa-onnx/pull/4133
- Unser Pfad `online-recognizer-transducer-nemo-impl.h`: `Reset()` setzt Encoder-Cache und Decoder zurück; `dither = 0`.
- Streaming sieht links begrenzt, rechts keinen Kontext; Batch sieht die ganze Äußerung (arXiv 2312.17279).

## 4. Testen und Messen

- cpal-Null-Host taugt nicht als Mock (`build_input_stream_raw` = `unimplemented!()`, cpal 0.18.1 `host/null/mod.rs`).
  Üblich: eigene Audioquelle hinter Trait mit Fake-Implementierung.
- Home Assistant: Test-WAVs im Code, Audio-Stream gepatcht, STT gemockt, Snapshot-Assertions;
  wyoming-satellite `--debug-recording-dir` speichert, was STT bekam.
- Pipecat: `run_test(...)` schiebt Frames durch die Pipeline (https://github.com/pipecat-ai/pipecat/blob/main/src/pipecat/tests/utils.py).
- BlackHole als Loopback für echten Gerätepfad, lokal; CI-Tauglichkeit nicht verifiziert.
- Metriken: RTF (sherpa-onnx), TTFS p50/p95/p99 (pipecat-stt-benchmark), Start-/End-Event je Messung definieren.
- Echte Stimmen: FLEURS de_de CC-BY-4.0 (kleine Fixtures dürfen ins Repo, mit Attribution);
  Common Voice DE CC0, aber Nutzungsbedingungen verbieten Re-Hosting → nur lokal per Skript.

## Offene Punkte (nicht verifiziert)

- Tatsächliche Startlücke in talker je Gerät (eingebaut, USB, AirPods).
- Ob der orange Punkt nach cpal `pause()` erlischt.
- Wirkung von Bug D und Hypothese E auf die WER.
- BlackHole auf GitHub-macOS-Runnern.
