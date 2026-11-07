# ADR-0011 — Logging: `log`-Fassade, eigener nicht blockierender Datei-Logger mit Rotation

- Status: akzeptiert
- Datum: 2026-11-07
- Kontext: Ticket-0038, `docs/requirements/diagnostics.md` (REQ-DIAG-001 … 011)
- Baut auf: ADR-0001 (Hot-Path ohne Panic), `docs/stability.md` (Config-Vertrag)

## Kontext und Problemstellung

talker loggt heute mit `eprintln!` (53 Stellen in `src/`). Startet die App als
Bundle (Finder, Login-Item), geht stderr ins Leere. Gefordert sind: eine
Logdatei `~/Library/Logs/talker/talker.log`, Rotation bei 1 MB mit höchstens
4 älteren Dateien, Detailgrad zur Laufzeit umschaltbar (`log_level`,
`TALKER_LOG`), kein Panic und kein Blockieren im Hot-Path, Fallback auf stderr,
wenn das Verzeichnis nicht beschreibbar ist.

Fakten aus dem Dependency-Graphen (Cargo.lock, `cargo tree -i log`): die
Fassade `log` 0.4.33 (MIT OR Apache-2.0) ist schon transitiv im Baum, über
eframe/egui/egui-winit/egui_glow/epaint und rustls/ureq. `libc` 0.2.186
(MIT OR Apache-2.0) ist ebenfalls transitiv vorhanden.

## Entscheidung

**`log` wird direkte Dependency. Ein eigenes Modul `src/logging.rs` stellt den
einzigen `log::Log`-Logger der App. Es schreibt über einen begrenzten Kanal
auf einem eigenen Writer-Thread und rotiert selbst.**

Aufbau:

1. **Aufrufer** nutzen ausschließlich `log::{error,warn,info,debug}!`. Kein
   `eprintln!` in `src/` außer dem einen stderr-Hinweis in `logging` selbst
   (REQ-DIAG-010). Im Audio-Callback wird nie geloggt.
2. **Logger** (`impl log::Log`): `enabled()` prüft den Detailgrad (siehe
   Filter), `log()` formatiert die Zeile im aufrufenden Thread und übergibt sie
   per `std::sync::mpsc::SyncSender::try_send` an einen Kanal mit fester
   Kapazität (1024 Zeilen). Ist der Kanal voll, wird die Zeile verworfen und
   ein `AtomicU64` gezählt. Ist der Writer weg (`Disconnected`), wird ebenfalls
   verworfen. `log()` wartet nie und paniciert nie.
3. **Writer-Thread** (`talker-log`): liest den Kanal, schreibt in einen
   `LogSink`. Wurden Zeilen verworfen, schreibt er zuerst einmal
   „… N Logzeilen verworfen (Puffer voll)". Schreibfehler zur Laufzeit gehen
   einmalig als Hinweis auf stderr, danach wird weiter versucht.
4. **Rotation** (`RotatingFile`, implementiert `LogSink`): 1 MB = 1 000 000
   Byte. Würde eine Zeile die Datei über 1 000 000 Byte bringen, wird vorher
   rotiert: `talker.4.log` löschen, `talker.3.log` → `talker.4.log` …
   `talker.log` → `talker.1.log`, neue `talker.log` öffnen. Eine Datei ist
   damit höchstens 1 000 000 Byte groß (Ausnahme: eine einzelne Zeile, die
   allein größer ist, steht in einer frischen Datei). Die aktuelle Größe wird
   beim Öffnen aus den Metadaten gelesen und dann mitgezählt, nicht pro Zeile
   per `stat`. Keine Löschung nach Alter.
5. **Detailgrad**: Der globale Grad ist `log::set_max_level` (ein Atomic in
   `log`, Quelle: `log-0.4.33/src/lib.rs`, `set_max_level` →
   `MAX_LOG_LEVEL_FILTER.store`). Umschalten zur Laufzeit ist damit ein Store.
   Filter in `enabled()`: Ziele unter `talker` folgen dem gewählten Grad,
   fremde Crates (egui, winit, rustls, ureq …) höchstens `warn`, damit sie die
   Mengenregel auf `info` nicht verletzen (REQ-DIAG-009).
6. **`TALKER_LOG`** wird einmal bei `init` gelesen: gültiger Wert
   (`error`/`warn`/`info`/`debug`, Groß/Klein egal, Leerraum getrimmt)
   übersteuert für den Prozess; leerer Wert gilt als nicht gesetzt; ungültiger
   Wert → Config-Wert plus eine Warnung (REQ-DIAG-006/-007).
7. **Fallback**: Lässt sich `~/Library/Logs/talker/` nicht anlegen oder
   `talker.log` nicht öffnen, schreibt `init` einen Hinweis auf stderr und
   installiert den Logger mit einem stderr-Sink (Terminal-Start sieht die
   Zeilen weiter, Bundle-Start verliert sie wie heute). Die App läuft weiter.
8. **Zeitstempel** je Zeile in Ortszeit mit Millisekunden
   (`2026-11-07 03:44:12.123`), formatiert mit `libc::localtime_r`
   (thread-sicher laut POSIX; `libc` wird direkte Dependency). Schlägt die
   Umrechnung fehl, wird UTC mit Suffix `Z` geschrieben.

Zeilenformat: `<Zeitstempel> <LEVEL> <modul>: <text>`, Modul ohne Präfix
`talker::`. Log-Texte sind kein Vertrag (`docs/stability.md`).

## Betrachtete Optionen

1. **`tracing` + `tracing-subscriber` + `tracing-appender`.** `tracing-appender`
   rotiert nur zeitbasiert (`Rotation::MINUTELY/HOURLY/DAILY/WEEKLY/NEVER`,
   Quelle: `tracing-appender-0.2.5/src/rolling.rs`), nicht nach Größe; die Größenrotation müsste trotzdem selbst geschrieben werden.
   Laufzeit-Umschaltung bräuchte `reload`-Layer. Spans bringen hier keinen
   Nutzen, weil die Messzeile eine fertige Zeile ist. Verworfen: mehr Gewicht,
   die schwierige Teilaufgabe bleibt.
2. **`log` + `flexi_logger` oder `log4rs`.** Beide können Größenrotation und
   Laufzeit-Grad. `flexi_logger` 0.31.10 schreibt bei `Naming::Numbers` in
   eine Datei mit Infix `rCURRENT`, bei `NumbersDirect` in `…_r00000` usw.
   (docs.rs, `enum Naming`), also nicht in `talker.log`. Das Verhalten bei
   vollem Puffer im asynchronen Modus müsste erst nachgewiesen werden. `log4rs`
   schreibt nach unserem Kenntnisstand synchron im aufrufenden Thread, also mit
   Lock und Datei-I/O im Hot-Path (nicht am Quellcode geprüft).
   Beide ziehen zusätzliche Abhängigkeiten nach (Zeit, Regex, Glob), die
   `cargo deny` und `THIRD-PARTY.html` erweitern. Verworfen: es bleibt mehr zu
   prüfen, als selbst zu schreiben ist.
3. **`log` + eigener Logger (gewählt).** Rund 200 Zeilen in einem Modul. Die
   kritischen Eigenschaften (nie blockieren, Rotation an der Grenze, Fallback)
   liegen im eigenen Code und sind direkt testbar. Keine neue Lizenz im Baum.

## Konsequenzen

- Neue direkte Dependencies `log` und `libc`, beide schon transitiv im Baum:
  keine neue Lizenz für `cargo deny`. `THIRD-PARTY.html` listet laut
  `docs/licensing.md` schon alle Abhängigkeiten des Builds, also auch diese
  beiden.
- Unter Last gehen Logzeilen eher verloren, als dass die Dictation wartet.
  Verluste werden in der Datei gezählt gemeldet.
- Ausgaben nativer Bibliotheken direkt auf fd 2 (sherpa-onnx, llama.cpp)
  laufen nicht über diesen Logger und bleiben im Bundle unsichtbar. Das deckt
  dieses ADR nicht ab.
- Panic-Meldungen nimmt der Logger nicht auf (kein Panic-Hook). Das gehört
  nicht zu Ticket-0038.
- Der Logger ist prozessweit (`log::set_boxed_logger` geht nur einmal). Tests
  prüfen `Logger`, `RotatingFile` und die Kanal-Logik ohne globale
  Installation. Logzeilen der Pipeline prüft eine Test-Hilfe, die pro Thread
  mitschreibt.
