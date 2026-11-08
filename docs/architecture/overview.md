# Architektur-Überblick — talker

Leichtgewichtige Architektur-Doku statt vollem arc42 (Begründung: [ADR-0004](../adr/adr-0004-doku-format-ears-und-adr-diagramme.md)).
Entscheidungen stehen in [`docs/adr/`](../adr/), Begriffe in [`CONTEXT.md`](../../CONTEXT.md) —
hier nur, wie die Teile zusammenhängen: Bausteine, Laufzeit, Kontext.

## Bausteinsicht

Module nach Abhängigkeitsrichtung geschichtet — Pfeile zeigen "hängt ab von".
`main.rs` ist der Composition Root und verdrahtet alles; kein anderes Modul
kennt `main.rs`.

```mermaid
flowchart TB
  subgraph main["main.rs (Composition Root)"]
  end

  subgraph orchestration["Orchestrierung"]
    pipeline["pipeline<br/>DictationWorker · PttSession · resolve_mode"]
    indicator["indicator<br/>Phase-Zustandsmaschine"]
    diagnostics["diagnostics<br/>DictationTrace · Messzeile · DebugCapture"]
  end

  subgraph presentation["Präsentation"]
    ui["ui<br/>Settings-Fenster (egui)"]
    tray["tray<br/>Menüleisten-Icon"]
    overlay["overlay<br/>Aufnahme-Overlay"]
  end

  subgraph engines["Engines / IO"]
    audio["audio<br/>AudioSource (Mic/WAV) · Recording · HostInstant"]
    stt["stt<br/>Transcriber-Trait"]
    cleanup["cleanup<br/>LlmCleaner-Trait"]
    injection["injection<br/>Text-Einfügen"]
    clipboard["clipboard"]
    models["models<br/>Downloader/ModelState"]
    hotkey["hotkey<br/>CGEventTap"]
    login_item["login_item"]
    permissions["permissions"]
  end

  subgraph foundation["Fundament"]
    config["config<br/>Config, CleanupMode-Nutzung"]
    vocab_match["vocab_match<br/>Kölner Phonetik"]
    error["error<br/>TalkerError"]
    logging["logging<br/>log-Backend · Rotation · Detailgrad"]
  end

  main --> pipeline & ui & tray & overlay & hotkey & permissions & models & logging

  pipeline --> cleanup & config & injection & models & stt & audio & vocab_match & error & diagnostics
  diagnostics --> audio & config & cleanup
  indicator --> audio & cleanup
  ui --> config & indicator & login_item & models & permissions & audio & hotkey & injection & logging
  tray --> cleanup & indicator & error
  overlay --> config & indicator

  audio --> error
  stt --> error
  cleanup --> error
  injection --> clipboard & error
  clipboard --> error
  models --> cleanup & error
  hotkey --> config & error
  login_item --> error
  config --> cleanup & error
  logging --> config

  classDef deep fill:#0f172a,color:#fff,stroke:#0f172a;
  class pipeline deep
```

`pipeline` ist das mit Abstand am meisten abhängige Modul (deep module) —
`DictationWorker` (STT+Cleanup-Lifecycle) und `PttSession` (Press/Release)
bündeln die Domänenlogik hinter einem kleinen Interface; `main.rs` bleibt
Verdrahtung. `error` und `config` sind das Fundament — von fast jedem Modul
direkt oder indirekt referenziert. `logging` ist das `log`-Backend (ADR-0011):
alle Module loggen über die `log`-Makros, ohne `logging` zu kennen; nur
`main.rs` (Init) und `ui` (Detailgrad, „Log im Finder zeigen") hängen direkt
davon ab. `audio` kapselt die Herkunft der Blöcke hinter `AudioSource`
(Mikrofon oder WAV, ADR-0012); `MicCache` merkt Gerät und Format zwischen
Tastendrücken, das erste Signal liest das Overlay lockfrei (ADR-0014).

## Laufzeitsicht — eine Utterance (CONTEXT.md)

Batch-Modell (v1): Text wird komplett erst nach dem Loslassen der PTT-Taste
eingefügt.

```mermaid
sequenceDiagram
    actor User
    participant Hotkey as hotkey (CGEventTap)
    participant Session as pipeline::PttSession
    participant Audio as audio (cpal)
    participant Worker as pipeline::DictationWorker
    participant STT as stt::Transcriber
    participant Vocab as vocab_match
    participant Cleanup as cleanup::LlmCleaner
    participant Injection as injection
    participant Indicator as indicator::Indicator
    participant TargetApp as Ziel-App

    User->>Hotkey: PTT-Taste drücken
    Hotkey->>Session: press(HostInstant::now())
    Session->>Session: Diktat-Nr. vergeben, Setup-Gate prüfen (Modell ready?)
    Session->>Session: Frontmost-App erfassen (Kontext-Awareness)
    Session->>Audio: start(MicSource + MicCache) — gemerktes Gerät, Callback stempelt Blöcke (HostInstant)
    Session->>Indicator: mic_started(level, signal, mode) — Balken bleiben
    Audio-->>Indicator: erstes Signal (Atomic, im Overlay-Takt gelesen) → Wellen
    Note over User,Audio: Nutzer spricht — Pegel live im Overlay

    User->>Hotkey: PTT-Taste loslassen
    Hotkey->>Session: release()
    Session->>Audio: stop() → Captured (Rohpuffer, erster Block/Signal)
    Session->>Session: Mindestlänge prüfen (MIN_UTTERANCE_MS)
    Session->>Worker: (pcm, frontmost) über Channel
    Worker->>STT: transcribe(pcm) → Raw Transcript
    Worker->>Vocab: apply(raw, vocabulary) [phonetic_matching]
    alt Cleanup-Modus ≠ Roh
        Worker->>Cleanup: clean_with_fallback(text)
        Cleanup-->>Worker: Cleaned Transcript (oder Fallback auf Raw)
    end
    Worker->>Injection: inject(text)
    Injection->>Injection: Clipboard sichern → schreiben → Cmd+V → wiederherstellen
    Injection->>TargetApp: Text erscheint im Fokus
    Worker->>Indicator: finish_ok() / fail(hint)
    Worker->>Worker: Messzeile loggen (info), ggf. Debug-WAVs schreiben
```

Tray und Overlay lesen `Indicator` unabhängig auf zwei Uhren (60-fps-Timer
bzw. egui-Repaint) — kein Teil dieses Sequenzflusses, siehe Code-Kommentare
in `tray.rs`/`overlay.rs` für die Idempotenz-Details.

Jede Dictation endet mit genau einer Messzeile (REQ-DIAG-012). Endet sie
vorzeitig (gesperrt, Mikrofon-Fehler, abgebrochen, zu kurz), loggt
`PttSession` die Zeile, sonst der Worker. Logzeilen gehen über einen
begrenzten Kanal an den Writer-Thread `talker-log`; kein Aufrufer wartet auf
Datei-I/O (ADR-0011).

## Kontextsicht — talker gegenüber macOS und der Außenwelt

```mermaid
flowchart LR
    User(("Nutzer"))
    TargetApp["Fokussierte Ziel-App<br/>(beliebig)"]

    subgraph talker["talker"]
        core["Dictation-Pipeline"]
    end

    CoreAudio["CoreAudio<br/>(Mikrofon)"]
    EventTap["CGEventTap<br/>(Accessibility-Permission)"]
    Pasteboard["NSPasteboard<br/>(Clipboard)"]
    StatusItem["NSStatusItem<br/>(Menüleiste)"]
    FS["Lokales Dateisystem<br/>(config.toml, Modelle,<br/>~/Library/Logs/talker/,<br/>opt-in TALKER_DEBUG_WAV_DIR)"]
    ModelHosts["GitHub / Hugging Face<br/>(Modell-Download, einmalig, mit Consent)"]

    User -- "PTT halten + sprechen" --> EventTap
    EventTap --> core
    CoreAudio -- "PCM-Audio" --> core
    core -- "Text via Cmd+V" --> Pasteboard
    Pasteboard --> TargetApp
    core <--> StatusItem
    core <--> FS
    core -. "nur beim Erst-Start,<br/>nach Lizenz-Zustimmung" .-> ModelHosts

    classDef net stroke:#dc2626,stroke-width:2px,stroke-dasharray: 4 4;
    class ModelHosts net
```

Kein laufender Netzwerk-Egress außer dem einmaligen, zustimmungspflichtigen
Modell-Download — Audio und Transkripte verlassen den Rechner nie
(Nachweis: [`docs/security-audit.md`](../security-audit.md)). STT
(`NemotronTranscriber`) und Cleanup (`GemmaCleaner`) laufen beide in-process,
kein Server, kein Daemon (ADR-0001, ADR-0003).
