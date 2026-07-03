# ADR-0001 — Rust + macOS-nativ mit trait-basiert austauschbaren Engines

- Status: akzeptiert
- Datum: 2026-07-02
- Kontext: siehe `CONTEXT.md`

## Kontext und Problemstellung

talker ist ein lokal laufender Diktier-Assistent für macOS (quelloffene, voll on-device laufende Alternative zu Cloud-Dictation). Die schweren Bausteine — Spracherkennung (STT) und LLM-Cleanup — existieren primär als C/C++-Bibliotheken (whisper.cpp, sherpa-onnx, llama.cpp). Zu entscheiden ist die Fundament-Kombination: Implementierungssprache, Plattform-Bindung und wie stark wir uns an konkrete STT-/LLM-Engines binden. Diese Entscheidung ist teuer zu revidieren, weil sie Sprache, Build-Toolchain und Modulschnitt der gesamten App festlegt.

## Entscheidungstreiber

- Rock-solid, langfristig wartbar, erstklassige Fehlerbehandlung, niedrige/planbare Latenz (Realtime-Diktat).
- Nur macOS, aktuellste Version — keine Cross-Platform-Ambitionen.
- Modell- und Engine-Landschaft ist volatil (neue Modelle wie gemma4:e2b erscheinen laufend); wir dürfen uns nicht hart an eine Engine binden.

## Betrachtete Optionen

1. **Swift + native macOS-Frameworks** (WhisperKit/CoreML, Foundation).
2. **Rust + macOS-native APIs via FFI**, ML-Engines via C-FFI, austauschbar hinter Traits.
3. **Tauri (Rust-Core + Web-UI)** — wie die Referenz Handy.

## Entscheidung

**Option 2: Rust als Implementierungssprache, direkte Nutzung nativer macOS-APIs, ML-Engines (STT, LLM) hinter Rust-Traits (`Transcriber`, `LlmCleaner`) gekapselt.** Konkrete Engine-/Modellwahl (Parakeet, gemma4:e2b, whisper.cpp, Ollama vs eingebettetes llama.cpp) ist bewusst hinter diese Traits verlagert und damit austauschbar, ohne den Rest umzubauen.

## Begründung

- Rust liefert die geforderte Fehlerbehandlung (`Result`, kein GC, keine Panics im Hot-Path) und planbare Latenz idiomatisch. Die ML-Kernbibliotheken sind ohnehin C/C++ und werden per FFI eingebunden — dieselbe Grenze hätte Swift auch. Rust gewinnt bei Wartbarkeit und Kontrolle über diese Grenze.
- macOS-nativ direkt (kein Cross-Platform-Layer): einfacher, da Scope ausdrücklich nur macOS ist.
- Trait-Abstraktion entkoppelt die volatile Engine-/Modellwahl von der stabilen Architektur — Wechsel (z.B. Ollama → eingebettetes llama.cpp, Parakeet → whisper.cpp) bleiben lokal begrenzt.

## Konsequenzen

- **Positiv:** stabile, wartbare Basis; Engine-/Modellwechsel billig; Referenz Handy zeigt, dass genau dieser Rust-Stack lokal funktioniert.
- **Negativ:** Native macOS-APIs (CoreAudio, CGEventTap, Accessibility) müssen aus Rust via FFI/Bindings angebunden werden — mehr Boilerplate als in Swift. Kein Zugriff auf Swift-only-Komfort wie WhisperKit ohne eigene Bindung.
- **Verworfen:** Swift (schwächere Latenz-/Fehler-Kontrolle für unseren Anspruch, obwohl native APIs leichter), Tauri (Web-UI-Overhead nicht nötig — v1 hat kaum UI, primär Menüleiste/Hotkey).
