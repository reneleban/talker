# ADR-0014 — Gerätecache und erstes Signal beim Mikrofon-Start

- Status: akzeptiert
- Datum: 2026-11-08
- Kontext: Ticket-0039, `docs/requirements/dictation-core.md` (REQ-DICT-016, -022, -023, -028 … -030),
  `docs/ticket/paket-mikrofon-start/research.md` (Befunde A, C, C')
- Baut auf: ADR-0012 (Audioquelle, gemeinsame Uhr) und ändert deren `OnBlock`-Typ

## Kontext und Problemstellung

Zwei Schwächen beim Mikrofon-Start:

1. **Wellen zu früh (C').** `on_ready` meldet nach `audio::start`, also sobald
   `play()` zurückkehrt. Ob dann schon Audio fließt, ist offen. Der Nutzer spricht
   zu früh. Das Overlay soll erst mit dem ersten Signal (REQ-DIAG-014) auf Wellen
   schalten. Der Audio-Callback darf dafür weder sperren noch allozieren noch loggen.
2. **Teurer Startpfad (A).** Jeder Tastendruck zählt bei gesetztem `mic_device`
   alle Geräte auf. Für jedes Gerät ruft `description()` die unterstützten Ein- und
   Ausgabekonfigurationen ab. Danach folgen Format und Gerätename. Gerät und Format
   sollen zwischen zwei Drücken gemerkt werden, ohne einen Stream offen zu halten.
   Wechsel des Geräts oder Formats muss der nächste Druck trotzdem erkennen.
   Scheitert der Start, versucht talker es genau einmal neu.

Folgetickets bauen darauf auf: 0040 (Mikrofon warm halten) braucht die gemerkte
Geräte-Referenz, 0041 (Latenz) braucht das Bereit-Signal. Deshalb ist das ein ADR.

Fakten (Primärquellen):

- cpal 0.18.1 `host/coreaudio/macos/enumerate.rs`, `default_input_device`: genau
  ein `AudioObjectGetPropertyData(kAudioHardwarePropertyDefaultInputDevice)`, keine
  Aufzählung.
- cpal 0.18.1 `host/coreaudio/macos/device.rs`:
  - `default_config`: liest `kAudioDevicePropertyStreamFormat` und die
    Puffergrößen-Spanne, also zwei Property-Abfragen.
  - `id()`: liest `kAudioDevicePropertyDeviceUID`. `cpal::DeviceId` hat
    `PartialEq` (`src/lib.rs`).
  - `description()`: ruft `supported_input_configs()` und
    `supported_output_configs()` auf. Das ist teuer, weil die Konfigurationen
    abgefragt werden. Bei einer Aufzählung per Name passiert das für jedes Gerät.
  - `build_input_stream_raw`: setzt zuerst per `set_physical_format` bzw.
    `set_sample_rate` das Hardwareformat auf die übergebene Config, dann folgen
    AUHAL-Init und `initialize()`. Mit einem veralteten Format würde talker also
    die Geräteeinstellung des Nutzers zurückstellen. Ein Formatwechsel muss
    deshalb vor dem Start erkannt werden.
  - `Device` ist `Clone` und enthält nur `AudioDeviceID` und ein Flag. Die
    `AudioDeviceID` ist `pub(crate)`. Der Property-Listener-Helfer
    (`property_listener.rs`) ist ebenfalls nicht öffentlich.
  - Kein Hog Mode: In `src/host/coreaudio/` gibt es keinen Treffer für `hog`.
- `std::sync::mpsc::Sender<T>` ist seit Rust 1.72.0 `Sync` für `T: Send`
  (std-Doku, `impl<T: Send> Sync for Sender<T>`).
- Wie lange eine CoreAudio-Property-Abfrage dauert, ist nicht gemessen.

## Entscheidung

### 1. Erstes Signal: lockfreier Griff, abgefragt im Overlay-Takt

- `audio::SignalHandle` (`Clone`) teilt die bestehenden `MarkCells` (ADR-0012).
  `fn arrived(&self) -> bool` liest `first_signal` mit `Relaxed`.
  `Recording::signal() -> SignalHandle`.
- Der Callback bleibt unverändert. Er setzt die Zelle schon heute mit einem
  `compare_exchange`, ohne Lock, Allokation oder Log. **Es gibt keine neue Meldung
  aus dem Callback.**
- Der 60-fps-Timer des Overlays ruft `Indicator::tick` auf. Dort wird die Phase
  `Starting` zu `Recording`, sobald `arrived()` wahr ist. Die Umschaltung kommt
  höchstens einen Frame (≈ 16 ms) nach dem ersten Signal.
- `MicReady::Recording` trägt zusätzlich `signal: SignalHandle`.
  `Indicator::start_recording` wird zu `mic_started(level, signal, mode)`. Die
  Phase bleibt dabei `Starting`, und `phase_since` behält den Zeitpunkt des
  Tastendrucks.
- 5 s ohne erstes Signal ergibt `Error("Mikrofon antwortet nicht")`
  (REQ-DICT-023). Solange eine Aufnahme offen ist, blendet sich der Fehler nicht
  aus, und ein späteres Signal schaltet nicht mehr auf Wellen. Erst
  `transcribing()` (Loslassen) bzw. `cancel()` beendet ihn. Pipeline und Aufnahme
  merken davon nichts.
- In `tick` wird das Signal **vor** dem Timeout geprüft.

### 2. Gerätecache: Referenz und Format merken, je Druck günstig prüfen

- `audio::MicCache` (`Clone`, `Arc<Mutex<Option<CachedMic>>>`) lebt im
  `SourceFactory`-Closure in `main.rs` und überdauert die Tastendrücke. Gesperrt
  wird er nur auf dem Start-Thread je Druck, nie im Audio-Callback und nie im
  Event-Tap-Callback.
- `CachedMic` enthält `wanted: Option<String>` (das `mic_device`, für das
  aufgelöst wurde), die Geräte-Referenz (`cpal::Device`), die UID, das Format
  (Kanäle, Rate, Sample-Format) und den Namen fürs Log. **Kein Stream.**
- Die Prüfung je Druck (`MicSource::open`, Start-Thread) läuft so:
  - `wanted` unterscheidet sich von `mic_device` → neu ermitteln.
  - Bei leerem `mic_device` wird das Standardgerät gelesen. Weicht seine UID ab
    → neu ermitteln.
  - Bei gesetztem `mic_device` wird die UID des gemerkten Geräts gelesen. Ein
    Fehler heißt „Gerät weg" → neu ermitteln.
  - Weicht das aktuelle Format (`default_input_config`) ab → neu ermitteln.
  - Sonst nutzt talker Gerät und Format aus dem Cache, ohne Aufzählung und ohne
    `description()`.
- Wurde ein benanntes Gerät nicht gefunden und talker ist auf das Standardgerät
  ausgewichen, wird **nicht** gemerkt. Jeder Druck sucht dann wieder per Name, wie
  heute. So greift ein später angestecktes Gerät sofort.
- **Genau ein Neuversuch (REQ-DICT-030):** Scheitert der Start mit Daten aus dem
  Cache, leert talker den Cache, ermittelt einmal neu und startet noch einmal.
  Scheitert auch das, oder kamen die Daten nicht aus dem Cache, geht der Fehler
  nach oben wie heute (REQ-DICT-006). Die Grenze von 2 Versuchen folgt aus der
  Struktur, es gibt keine Schleife.
- **Naht für Tests:** Ein crate-interner Trait `MicBackend` kapselt die sechs
  CoreAudio-Zugriffe: Standardgerät, Suche per Name (Aufzählung), UID, Name,
  Format und Öffnen. Implementiert wird er von `CpalBackend` (App) und einem
  zählenden Fake (Tests). `MicSource` ist generisch mit Default `CpalBackend`.
  Nach außen bleibt `AudioSource` (ADR-0012) die Schnittstelle.

### 3. `OnBlock` wird teilbar

```rust
pub type OnBlock = Arc<dyn Fn(&[f32], HostInstant) + Send + Sync>;
```

Der Neuversuch braucht den Block-Verbraucher ein zweites Mal. cpal verbraucht die
Closure aber auch dann, wenn `build_input_stream` scheitert. Der Verbraucher aus
`audio::start` mutiert nur über `Arc`s, `Atomic`s und `Mutex`, ist also schon
heute `Fn`. Seit Rust 1.72 ist auch der Live-`Sender` `Sync`. Der Aufruf über
`Arc` braucht keine Sperre. Der I16-Scratch-Puffer bleibt in der cpal-Closure von
`MicSource`.

## Betrachtete Optionen

Bereit-Signal:

1. **Abfrage im Overlay-Takt über einen atomaren Griff (gewählt).** Der Callback
   bleibt unverändert, und mit eingespeisten Zeitpunkten ist das deterministisch
   testbar. Die Umschaltung kann bis zu einen Frame zu spät kommen.
2. **Meldung aus dem Callback über einen Kanal oder `dispatch_async` an den
   Main-Thread.** Das alloziert bzw. sperrt im Echtzeit-Thread. Verworfen.
3. **Wächter-Thread mit Condvar.** `notify` braucht den zugehörigen Mutex im
   Callback. Verworfen.

Erkennung von Geräte- und Formatwechseln:

1. **Günstige Prüfung je Druck (gewählt):** Standardgerät bzw. UID plus Format,
   zusammen etwa 4 Property-Abfragen. Sie kommt mit der öffentlichen cpal-API aus,
   braucht kein neues `unsafe` und ist über den Fake vollständig testbar.
2. **CoreAudio-Property-Listener** auf das Standard-Eingabegerät und auf
   Streamformat und `DeviceIsAlive` des Geräts. Ein Listener setzt ein
   Dirty-Flag, und ohne Wechsel kommt der Druck ohne jede Abfrage aus. Dafür
   bräuchte talker eigenes FFI (`AudioObjectAddPropertyListener`, Lebensdauer
   der Client-Daten, Ab- und Anmelden beim Gerätewechsel) und die
   `AudioDeviceID`, die cpal nicht herausgibt (UID→ID per
   `kAudioHardwarePropertyTranslateUIDToDevice`). Das sind deutlich mehr `unsafe`
   und mehr Testaufwand für ein paar eingesparte Abfragen. Verworfen, solange die
   Messzeile (Taste→Mikro bereit) keinen Bedarf zeigt.
3. **Kein Format prüfen, nur den Neuversuch bei Fehlern.** Ein Formatwechsel
   lässt den Start nicht scheitern. cpal würde das alte Format auf das Gerät
   zurückschreiben. Verworfen.

## Konsequenzen

- `Indicator::start_recording` entfällt zugunsten von `mic_started`, und
  `Indicator::tick` liest den `SignalHandle`. Das Tray färbt sich weiterhin erst
  in `Phase::Recording`, also ab dem ersten Signal.
- „Taste→Mikro bereit" in der Messzeile bleibt der Zeitpunkt nach
  `audio::start`, ohne Bedeutungswechsel. Das erste Signal steht schon heute in der
  Zeile.
- Je Druck bleiben etwa 4 CoreAudio-Property-Abfragen sowie Stream-Aufbau und
  `play()`. Den Stream-Aufbau zu sparen ist Sache von 0040 (vorgebauter Stream),
  das auf `MicCache` aufsetzen kann.
- Zwischen zwei Aufnahmen ist kein Stream offen, und kein Gerät wird exklusiv
  belegt. Gemerkt wird nur die `AudioDeviceID` in `cpal::Device`.
- Fakten-Lücke: Ob ein Start mit einer veralteten `AudioDeviceID` (Gerät
  abgezogen) als Fehler zurückkommt oder still nichts liefert, ist nicht belegt.
  Der Fall wird schon vorher über die UID-Abfrage abgefangen. Liefert ein Gerät
  still nichts, greift REQ-DICT-023 (5 s, „Mikrofon antwortet nicht").
