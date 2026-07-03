# ADR-0003 — Eingebettetes llama.cpp mit gemma4:e2b für den Cleanup (kein Server)

- Status: akzeptiert
- Datum: 2026-07-02
- Kontext: siehe `CONTEXT.md`
- Belegt durch: Spike — gemma4:e2b läuft eingebettet via `llama-cpp-2` auf Apple Silicon (verifiziert 2026-07-02)
- Baut auf: ADR-0001 (Rust, self-contained, austauschbare Engines hinter Traits)

## Kontext und Problemstellung

Das `cleanup`-Modul führt ein kleines LLM lokal aus, um rohe Transkripte zu bereinigen (Füllwörter, Interpunktion, Formatierung). Zu entscheiden war, **wie** das LLM aus der Rust-App angesprochen wird: über einen laufenden lokalen Server (Ollama) oder eingebettet in den Prozess. Der Server-Weg widerspricht dem self-contained-Anspruch (externe Installation + Daemon). Der eingebettete Weg war attraktiver, aber es war unklar, ob das gewünschte, brandneue Modell `gemma4:e2b` dort bereits läuft.

## Entscheidungstreiber

- Self-contained, kein Hintergrund-Daemon, keine externe Installation (ADR-0001).
- Niedrige Cleanup-Latenz, planbarer Ressourcenverbrauch.
- Modell `gemma4:e2b` (edge, effective 2,3B) als Wunsch — Verfügbarkeit im eingebetteten Pfad musste verifiziert werden.

## Betrachtete Optionen

1. **Eingebettet: llama.cpp via `llama-cpp-2` (FFI).** Kein Server, Modell in-process.
2. **Ollama-Daemon (localhost HTTP).** Einfach, aber externe Runtime.
3. **Pure Rust (Candle/mistral.rs).** Kein C++, aber lückenhafter Modell-Support.

## Entscheidung

**Option 1: eingebettetes llama.cpp über `llama-cpp-2`, Modell `gemma4:e2b` (GGUF `ggml-org/gemma-4-E2B-it-GGUF`), kein Server.** Chat-Template zwingend mit `reasoning off`. Weiterhin hinter dem `LlmCleaner`-Trait (ADR-0001), sodass ein späterer Wechsel lokal begrenzt bleibt.

## Begründung (durch Spike-0001 belegt, 2026-07-02)

- GGUF offiziell verfügbar; llama.cpp unterstützt Gemma 4 seit Launch.
- Aus Rust via `llama-cpp-2` 0.1.150 auf Apple Silicon (M4 Max/Metal) verifiziert: Laden ~1,9 s, Prompt 405 t/s, Generierung ~90–100 t/s → Transkript-Cleanup <1 s (warm), ~5,1 GB RSS bei Q8_0 (mmap-dominiert).
- Damit ist der self-contained-Weg nicht nur bevorzugt, sondern nachweislich tragfähig — der Ollama-Fallback entfällt.

## Konsequenzen

- **Positiv:** keine externe Runtime/Daemon; alles in einer Binary; volle Kontrolle über Latenz und Modell-Handling; erfüllt ADR-0001.
- **Negativ:** Modell-Management (GGUF beziehen, ablegen, ggf. updaten) liegt bei uns. `reasoning off` ist ein nicht-offensichtlicher Pflicht-Schritt (sonst Thinking-Leak + hohe Latenz) — in Ticket/Prompt-0005 festgehalten.
- **Offen/nicht getestet:** Q4_K_M-Qualität (≈ halber Speicher), lange Transkripte (>4K Kontext). Bei Bedarf später messen.
- **Verworfen:** Ollama (widerspricht self-contained; nicht nötig, da eingebettet belegt funktioniert), Candle/mistral.rs (Support-Risiko, kein Mehrwert gegenüber llama.cpp).
