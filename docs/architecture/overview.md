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
  end

  subgraph presentation["Präsentation"]
    ui["ui<br/>Settings-Fenster (egui)"]
    tray["tray<br/>Menüleisten-Icon"]
    overlay["overlay<br/>Aufnahme-Overlay"]
  end

  subgraph engines["Engines / IO"]
    audio["audio<br/>Mikrofon-Capture"]
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
  end

  main --> pipeline & ui & tray & overlay & hotkey & permissions & models

  pipeline --> cleanup & config & injection & models & stt & audio & vocab_match & error
  indicator --> audio & cleanup
  ui --> config & indicator & login_item & models & permissions & audio & hotkey & injection
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

  classDef deep fill:#0f172a,color:#fff,stroke:#0f172a;
  class pipeline deep
```

`pipeline` ist das mit Abstand am meisten abhängige Modul (deep module) —
`DictationWorker` (STT+Cleanup-Lifecycle) und `PttSession` (Press/Release)
bündeln die Domänenlogik hinter einem kleinen Interface; `main.rs` bleibt
Verdrahtung. `error` und `config` sind das Fundament — von fast jedem Modul
direkt oder indirekt referenziert.

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
    Hotkey->>Session: press()
    Session->>Session: Setup-Gate prüfen (Modell ready?)
    Session->>Session: Frontmost-App erfassen (Kontext-Awareness)
    Session->>Audio: start(mic_device)
    Session->>Indicator: start_recording(mode)
    Note over User,Audio: Nutzer spricht — Pegel live im Overlay

    User->>Hotkey: PTT-Taste loslassen
    Hotkey->>Session: release()
    Session->>Audio: stop() → PCM
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
```

Tray und Overlay lesen `Indicator` unabhängig auf zwei Uhren (60-fps-Timer
bzw. egui-Repaint) — kein Teil dieses Sequenzflusses, siehe Code-Kommentare
in `tray.rs`/`overlay.rs` für die Idempotenz-Details.

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
    FS["Lokales Dateisystem<br/>(config.toml, Modelle)"]
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
(`ParakeetTranscriber`) und Cleanup (`GemmaCleaner`) laufen beide in-process,
kein Server, kein Daemon (ADR-0001, ADR-0003).
