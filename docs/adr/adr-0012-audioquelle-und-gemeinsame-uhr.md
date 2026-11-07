# ADR-0012 — Austauschbare Audioquelle und gemeinsame Host-Uhr

- Status: akzeptiert
- Datum: 2026-11-07
- Kontext: Ticket-0038, `docs/requirements/diagnostics.md` (REQ-DIAG-012 … 024),
  `docs/research/mikrofon-start-und-pre-roll.md`
- Baut auf: ADR-0001 (Engines hinter Traits), ADR-0009 (Live-Diktat, Block-Kanal)

## Kontext und Problemstellung

`audio::start(device, blocks)` baut einen cpal-Stream direkt auf. Ohne
Mikrofon lässt sich die Pipeline nicht von Ende zu Ende testen: der cpal-Null-Host
taugt nicht als Ersatz (`build_input_stream_raw` ist `unimplemented!()`,
cpal 0.18.1 `host/null/mod.rs`). Für die Messzeile brauchen Tastendruck,
erster Callback-Block, erstes Signal, Loslassen, Text und Einfügen Zeitpunkte
auf **einer** Uhr. Die Folgetickets 0039–0041 (Pre-Roll, Mikrofon warm halten,
Chunk-Größen) bauen auf dieser Schnittstelle auf. Deshalb ist sie ein ADR.

Fakten (Primärquellen):

- cpal 0.18.1 CoreAudio rechnet den Callback-Zeitstempel aus
  `AudioTimeStamp.mHostTime` mit `mach_timebase_info` in Nanosekunden um:
  `nanos = mHostTime * numer / denom` (u128), `StreamInstant::new(secs, nanos)`.
  Quelle: `cpal-0.18.1/src/host/coreaudio/mod.rs`, `host_time_to_stream_instant`;
  Aufruf im Input-Callback `src/host/coreaudio/macos/device.rs`.
- `Stream::now()` nutzt `mach_absolute_time()` mit derselben Umrechnung
  (`src/host/coreaudio/macos/mod.rs`). Die Doku verspricht dieselbe Zeitbasis
  wie die Callback-Zeitstempel (`src/traits.rs`, `fn now`). `now()` setzt aber
  einen Stream voraus, und beim Tastendruck gibt es noch keinen.
- `StreamInstant::as_nanos()` ist öffentlich (`src/timestamp.rs`).
- `man clock_gettime` (macOS): `CLOCK_UPTIME_RAW` „does not increment while the
  system is asleep. The returned value is identical to the result of
  mach_absolute_time() after the appropriate mach_timebase conversion is applied."
- `mach2` 0.6.0 (BSD-2-Clause OR MIT OR Apache-2.0) ist über cpal schon im Baum.

## Entscheidung

### 1. Gemeinsame Uhr: `audio::HostInstant`

Ein `Copy`-Newtype über Nanosekunden auf der Zeitbasis `mach_absolute_time`.
Umgerechnet wird genau wie in cpal (`ticks * numer / denom` in u128;
`mach_timebase_info` einmal in `OnceLock`).

- `HostInstant::now()` per `mach2::mach_time::mach_absolute_time()`
  (`mach2` wird direkte Dependency).
- `From<cpal::StreamInstant>` über `as_nanos()`.
- Im Mikrofon-Callback gilt `InputCallbackInfo::timestamp().callback` als
  Zeitpunkt des Blocks. `capture` wird nicht verwendet: cpal schätzt den Wert
  aus Puffergröße plus Gerätelatenz und setzt bei Unterlauf `ZERO`.
- Den Tastendruck stempelt der Event-Tap-Callback beim Eintritt mit
  `HostInstant::now()`. `CGEventGetTimestamp` wird nicht verwendet, weil seine
  Einheit nicht belegt ist. Die Zeit zwischen Taste und Tap-Callback ist damit
  nicht Teil der Messung.
- `std::time::Instant` bleibt für alles außerhalb der Messzeile, etwa für
  den Indicator. Zwischen den beiden Uhren wird nie umgerechnet.

### 2. Audioquelle: schmaler Erzeuger-Trait, gemeinsame Aufnahme dahinter

Der Trait kapselt nur, **woher** Blöcke kommen. Alles, was beide Quellen
gemeinsam brauchen (Puffer, Pegel, erster Block/erstes Signal, Live-Kanal,
Umrechnung auf 16 kHz), steckt einmal in `audio::Recording`.

```rust
pub trait AudioSource: Send {
    /// Öffnet die Quelle; ab Rückkehr ruft sie `on_block` je Block auf ihrem
    /// eigenen Thread auf (Mikrofon: CoreAudio-IO-Thread, WAV: eigener Thread).
    /// Drop von `Opened::stop` beendet die Lieferung; danach kommt kein Block mehr.
    fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened>;
}
pub type OnBlock = Box<dyn FnMut(&[f32], HostInstant) + Send>;
```

- **`MicSource`** (cpal, Verhalten wie heute: Gerätewahl per Name mit Fallback
  auf Default, F32/I16, `play()`).
- **`WavSource`** (Tests, `make latency-eval`): eigener Thread; Zielrate,
  Kanäle, Blockgröße (Frames), Startverzögerung und Takt (`Pace::RealTime`
  schläft bis zum Sollzeitpunkt jedes Blocks, `Pace::Immediate` liefert ohne
  Warten) sind einstellbar. Zeitstempel: bei `RealTime` `HostInstant::now()`,
  bei `Immediate` virtuell `epoch + Startverzögerung + Frames/Rate`. Damit
  sind Tests deterministisch, ohne zu schlafen. Ein `SourceDone`-Signal meldet
  dem Test, dass der letzte Block geliefert ist.
- Der Callback bleibt echtzeittauglich: keine Datei-I/O, kein Logging. Erster
  Block und erstes Signal (erster Block mit einem Sample ≠ 0) werden als
  `AtomicU64` einmalig gesetzt (0 = nicht gesetzt). Pegel, Puffer und
  Live-Kanal funktionieren wie heute.
- Der Live-Kanal trägt `Feed::Block(Block)` und zum Schluss
  `Feed::End(Ended)`. `End` schickt `Recording::finish_live` nach dem Drop des
  Streams, mit Loslass-Zeitpunkt und `Captured`. Der Worker hat damit alles
  in Reihenfolge auf einem Kanal.
- `PttSession` bekommt die Quelle über eine Factory. Die App übergibt
  `MicSource`, Tests übergeben `WavSource`. Der Start-Thread pro Tastendruck
  bleibt.

### 3. Debug-Mitschnitt ohne I/O im Callback

- **`<nr>-mic.wav`**: Quelle ist der Rohpuffer von `Recording` (interleaved,
  Geräteformat, ab dem ersten Block). Er wird schon heute in beiden Modi
  gefüllt. Geschrieben wird nach dem Einfügen auf dem STT-Worker-Thread.
- **`<nr>-model.wav`**: `stt::ModelTap` (geteilter Puffer). `NemotronTranscriber`
  leitet jedes `accept_waveform` über eine private Hilfsfunktion, die auch in
  den Tap schreibt. So landen Vor- und Nachlauf und die Stille nach jedem Reset
  sicher im Mitschnitt. Der Tap ist eine Eigenschaft des Adapters, der
  `Transcriber`-Trait bleibt unverändert (ADR-0001).
- Ohne `TALKER_DEBUG_WAV_DIR` gibt es weder `DebugCapture` noch `ModelTap`,
  und es wird nichts geschrieben (REQ-DIAG-022).

## Betrachtete Optionen

Uhr:

1. **`std::time::Instant` überall, auch im Callback.** Einfach, aber der
   Callback-Zeitstempel von CoreAudio ginge verloren. In Tests ließe sich
   „erster Block < erstes Signal" ohne Schlafen nicht deterministisch prüfen.
   Verworfen.
2. **`HostInstant` auf `mach_absolute_time`-Basis (gewählt).** Gleiche
   Zeitbasis wie cpal, belegt durch Quellcode und Manpage. Erlaubt virtuelle
   Zeitstempel in Tests.
3. **`clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`.** Laut Manpage gleichwertig,
   aber die Rundung kann von cpals Formel abweichen. Verworfen zugunsten
   bitgleicher Umrechnung.

Audioquelle:

1. **Trait über die ganze Aufnahme (`start`/`stop`/`level`/`format`).** Jede
   Quelle müsste Puffer, Pegel und Messpunkte selbst bauen, also doppelte
   Logik mit dem Risiko, dass Test und App sich unterscheiden. Verworfen.
2. **Schmaler Erzeuger-Trait plus gemeinsames `Recording` (gewählt).** Die
   App- und die Testquelle unterscheiden sich nur in der Blockerzeugung. Die
   geprüfte Logik ist dieselbe wie in der App.
3. **Nur rohe Blöcke als `Iterator` in die Pipeline.** Deckt den
   Batch-Pfad und `PttSession` nicht ab. Verworfen.

## Konsequenzen

- `audio::start(device, blocks)` wird zu `audio::start(source, live)`.
  `Recording::format()` wird zu `Recording::info()` (Format und Gerätename).
- Neue direkte Dependencies: `mach2` (schon im Baum), `hound` wechselt von
  dev- zu normaler Dependency (WAV lesen und schreiben, Apache-2.0). Mit ihr
  muss `THIRD-PARTY.html` neu erzeugt werden. `hound` paniciert bei Float mit
  `bits_per_sample ≠ 32` (`hound-3.5.1/src/write.rs`). Geschrieben wird nur
  32-bit Float.
- Der Event-Tap-Callback und die Callback-Messpunkte kosten je einen
  `mach_absolute_time`-Aufruf. cpal ruft ihn schon heute in jedem Callback auf.
  Die Kosten sind nicht gemessen.
- 0039 (Pre-Roll, Stream vorbauen) und 0040 (Mikrofon warm halten) ändern nur
  `MicSource` und `Recording`. Die Testquelle und die Messzeile bleiben.
