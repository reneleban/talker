# Dictation Core — fachliche Anforderungen (EARS)

Feature-Bereich: PTT, Audio-Capture, Spracherkennung (STT), Injection,
Aufnahme-Feedback/Overlay. Muster und ID-Konvention: [`README.md`](README.md).
Begriffe: [`CONTEXT.md`](../../CONTEXT.md).

## PTT & Aufnahme

- **REQ-DICT-001**: WENN der Nutzer die PTT-Taste drückt, soll das System
  zuerst die frontmost App (bundle-id) erfassen und danach die
  Mikrofon-Aufnahme starten, damit die App-Zuordnung stabil bleibt, auch wenn
  während der Utterance der Fokus wechselt.
  (`src/pipeline.rs:PttSession::press`, Ticket-0026/0038)
- **REQ-DICT-002**: WÄHREND das STT-Modell nicht einsatzbereit ist (Setup-Gate
  offen), soll das System einen Tastendruck der PTT-Taste sperren und einen
  sichtbaren Hinweis statt einer stillen Aufnahme liefern.
  (`src/pipeline.rs:PttSession::press`, `PressOutcome::Blocked`)
- **REQ-DICT-003**: Das System soll aufgenommenes Audio ausschließlich
  in-memory halten, ohne es auf Platte zu schreiben oder zu übertragen.
  (`src/audio.rs:Recording`, Ticket-0003 AK4)
- **REQ-DICT-004**: WENN der Nutzer die PTT-Taste loslässt, soll das System
  die Aufnahme beenden und das Audio als 16 kHz mono PCM bereitstellen
  (Downmix bei Mehrkanal, Resampling bei abweichender Gerätesamplerate).
  (`src/audio.rs:to_mono_16k`, `Recording::stop`, Ticket-0003 AK2)
- **REQ-DICT-005**: FALLS die aufgenommene Utterance kürzer als 300 ms ist,
  DANN soll das System sie als versehentlichen Tastendruck verwerfen, ohne sie
  an die Spracherkennung weiterzugeben.
  (`src/audio.rs:MIN_UTTERANCE_MS`, `src/pipeline.rs:PttSession::release` →
  `ReleaseOutcome::TooShort`, Ticket-0003 AK5)
- **REQ-DICT-006**: FALLS beim Start der Aufnahme kein Mikrofon verfügbar ist
  oder die Mikrofon-Permission fehlt, DANN soll das System die Aufnahme mit
  einem klaren Fehler abbrechen statt abzustürzen.
  (`src/audio.rs:start` → `TalkerError::Audio`, `PressOutcome::Failed`,
  Ticket-0003 AK3)
- **REQ-DICT-007**: FALLS das Stoppen einer laufenden Aufnahme fehlschlägt,
  DANN soll das System dies als sichtbaren Fehler melden statt die Utterance
  stillschweigend zu verlieren.
  (`src/pipeline.rs:PttSession::release` → `ReleaseOutcome::StopFailed`)
- **REQ-DICT-008**: WÄHREND eine Aufnahme läuft, soll das System pro
  Audio-Frame einen Live-Pegel (RMS) bereitstellen, den andere Komponenten
  lockfrei auslesen können.
  (`src/audio.rs:LevelHandle`, `rms`, Ticket-0007 AK2)

## Spracherkennung

- **REQ-DICT-009**: Das System soll 16 kHz mono PCM einer Utterance über ein
  austauschbares `Transcriber`-Interface on-device in einen Raw Transcript
  (unbearbeiteten Text) umwandeln, ohne Netzwerkzugriff.
  (`src/stt.rs:Transcriber`, `ParakeetTranscriber`, ADR-0001, Ticket-0004 AK1/AK4)
- **REQ-DICT-010**: FALLS eine Utterance einen leeren PCM-Puffer liefert oder
  die Spracherkennung einen leeren Transcript zurückgibt, DANN soll das System
  die Utterance als „nichts erkannt" verwerfen, ohne etwas einzufügen.
  (`src/stt.rs:ParakeetTranscriber::transcribe`, `src/pipeline.rs:process_utterance`
  → `Processed::Empty` → `Outcome::Rejected("Nichts erkannt")`)
- **REQ-DICT-011**: FALLS beim Laden des STT-Modells Modell-Dateien fehlen
  oder beschädigt sind, DANN soll das System einen klaren Fehler mit
  Ablagepfad-Hinweis liefern statt abzustürzen, und weiterhin auf ein zuvor
  geladenes Modell zurückfallen, falls eines aktiv war.
  (`src/stt.rs:ParakeetTranscriber::new`, `src/pipeline.rs:DictationWorker::handle`
  „alter Transcriber bleibt bei Fehler aktiv", Ticket-0004 AK3)
- **REQ-DICT-012**: FALLS die Spracherkennung selbst mit einem Fehler
  abbricht, DANN soll das System die Utterance als „Spracherkennung
  fehlgeschlagen" sichtbar verwerfen statt sie stillschweigend zu verlieren.
  (`src/pipeline.rs:process_utterance` → `Processed::SttFailed` →
  `Outcome::Rejected`)

## Injection

- **REQ-DICT-013**: WENN ein fertiger Text (Cleaned oder Raw Transcript)
  vorliegt, soll das System das Nutzer-Clipboard sichern, den Text via
  Cmd+V-Simulation in die aktuell fokussierte Target App einfügen und danach
  das ursprüngliche Clipboard wiederherstellen.
  (`src/injection.rs:inject`, Ticket-0002 AK2)
- **REQ-DICT-014**: FALLS das Setzen des Clipboard-Texts oder das Simulieren
  von Cmd+V fehlschlägt, DANN soll das System den Fehler melden UND das
  Nutzer-Clipboard trotzdem zuverlässig wiederherstellen (`RestoreGuard`,
  auch über `Drop` auf jedem Fehlerpfad).
  (`src/injection.rs:RestoreGuard`, `inject`)
- **REQ-DICT-015**: FALLS die Accessibility-Permission fehlt, DANN soll das
  System dies sichtbar (Menüleiste + Onboarding-Hinweis) anzeigen statt
  Tastendrücke stillschweigend ins Leere laufen zu lassen.
  (`src/main.rs` `accessibility`-Check, `tray.set_permission_warning`,
  Ticket-0002 AK4)

## Aufnahme-Feedback/Overlay

- **REQ-DICT-016**: WENN die Aufnahme startet, soll das System ein
  click-through, always-on-top Overlay auf dem Bildschirm der aktiven App
  einblenden, das eine auf den Live-Mikrofon-Pegel reagierende Waveform
  zeigt, ohne der Target App den Fokus zu entziehen.
  (`src/overlay.rs:Overlay::tick/render_waves`, `src/indicator.rs:Phase::Recording`,
  Ticket-0007 AK1/AK5/AK6)
- **REQ-DICT-017**: WÄHREND die Utterance nach dem Loslassen verarbeitet
  wird, soll das Overlay den Zustand „wird transkribiert" anzeigen, bis der
  Text eingefügt oder die Utterance verworfen wurde.
  (`src/indicator.rs:Indicator::transcribing`, `src/overlay.rs` Phase::Transcribing,
  Ticket-0007 AK3)
- **REQ-DICT-018**: WENN die Injection erfolgreich abgeschlossen ist, soll
  das Overlay kurz einen „erledigt"-Zustand zeigen und danach automatisch
  ausblenden.
  (`src/indicator.rs:Indicator::finish_ok`, `DONE_SHOW`-Timeout)
- **REQ-DICT-019**: FALLS irgendeine Stufe der Pipeline (STT, Cleanup,
  Injection) fehlschlägt oder eine Utterance verworfen wird, DANN soll das
  Overlay einen sichtbaren Fehler-/Hinweiszustand mit Klartext-Meldung zeigen
  statt kommentarlos zu verschwinden.
  (`src/indicator.rs:Indicator::fail`, `ERROR_SHOW`-Timeout, `src/main.rs`
  `PressOutcome::Blocked`/`ReleaseOutcome::StopFailed`/`Outcome::Rejected`,
  Ticket-0007 AK4)
- **REQ-DICT-020**: WÄHREND Modelle beim App-Start noch laden, soll das
  Overlay einen „Laden"-Zustand ohne Timeout anzeigen; ein PTT-Druck während
  des Ladens wechselt direkt in den Recording-Zustand, sobald das Setup-Gate
  es zulässt.
  (`src/indicator.rs:Indicator::loading/ready`, Test
  `ptt_during_loading_switches_to_recording`)
