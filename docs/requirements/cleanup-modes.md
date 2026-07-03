# Fachliche Anforderungen — Cleanup-Modi & Vokabular-Korrektur

EARS-Format nach [`README.md`](README.md). Begriffe (Cleanup, Cleanup-Modus,
Raw/Cleaned Transcript) in [`CONTEXT.md`](../../CONTEXT.md).

## Cleanup-Modi

- **REQ-CLEAN-001**: Das System soll genau vier Cleanup-Modi anbieten — Roh,
  Geschäftlich, Natürlich (Stil erhalten), LLM-optimiert —, von denen zu jedem
  Zeitpunkt genau einer aktiv ist. (`src/cleanup.rs:CleanupMode`)
- **REQ-CLEAN-002**: WENN der Modus Roh aktiv ist, soll das System den Raw
  Transcript ohne LLM-Aufruf unverändert als Cleaned Transcript verwenden.
  (`src/cleanup.rs:CleanupMode::uses_llm`, `build_prompt`)
- **REQ-CLEAN-003**: WENN der Modus Geschäftlich aktiv ist, soll das System
  Füllwörter und unsichere Floskeln entfernen, vollständige Sätze mit
  korrekter Interpunktion erzeugen und dabei Zahlen-, Datums- und
  Zeitangaben exakt wie diktiert übernehmen. (`src/cleanup.rs:mode_instruction`,
  Ticket-0010 AK)
- **REQ-CLEAN-004**: WENN der Modus Natürlich aktiv ist, soll das System nur
  Füllwörter sowie Interpunktion/Groß-Kleinschreibung korrigieren und dabei
  Wortwahl, Ton und Sprachstil des Sprechers unverändert erhalten. Ausnahme:
  siehe REQ-CLEAN-031 (Zahlen-Normalisierung).
  (`src/cleanup.rs:mode_instruction`, CONTEXT.md „Cleanup-Modus")
- **REQ-CLEAN-005**: WENN der Modus LLM-optimiert aktiv ist, soll das System
  aus dem Diktat einen klaren, copy-paste-fertigen Prompt formen (Rambling
  entfernt, technische Details exakt übernommen, mehrere Punkte als
  nummerierte Liste), ohne die diktierte Anweisung selbst auszuführen oder zu
  beantworten. (`src/cleanup.rs:mode_instruction`)
- **REQ-CLEAN-006**: FALLS das Diktat im Modus LLM-optimiert keine erkennbare
  Anweisung enthält, DANN soll das System den Text nur bereinigen (Füllwörter
  raus, Interpunktion), ohne Inhalte zu erfinden. (`src/cleanup.rs:mode_instruction`,
  Few-Shot „Kantine")
- **REQ-CLEAN-007**: Das System soll in jedem Nicht-Roh-Modus Fragen im
  Diktat nicht beantworten, sondern nur als bereinigten Fragesatz
  zurückgeben. (`src/cleanup.rs:mode_instruction`, alle drei LLM-Profile)
- **REQ-CLEAN-008**: Das System soll in jedem Modus in der Sprache des
  Diktats antworten und darf englischsprachiges Diktat nicht ins Deutsche
  übersetzen. (`src/cleanup.rs:mode_examples`, EVAL-0003-Regressionstest
  „we should ship the fix on monday")

## Ausfallsicherheit

- **REQ-CLEAN-009**: FALLS der Cleanup-Aufruf fehlschlägt (Fehler oder
  Timeout), DANN soll das System den unveränderten Raw Transcript einfügen
  und den Fallback über ein Flag sichtbar machen. (`src/cleanup.rs:clean_with_fallback`,
  CONTEXT.md „Cleanup")
- **REQ-CLEAN-010**: FALLS der Cleanup-Aufruf einen leeren Text liefert,
  DANN soll das System ebenfalls auf den Raw Transcript zurückfallen und den
  Fallback melden. (`src/cleanup.rs:clean_with_fallback`)
- **REQ-CLEAN-011**: FALLS das Cleanup-Modell-File beim Laden fehlt, DANN
  soll das System einen eindeutigen Fehler statt eines Absturzes liefern.
  (`src/cleanup.rs:GemmaCleaner::new`)
- **REQ-CLEAN-012**: WÄHREND ein vorheriger Ladeversuch des Cleanup-Modells
  fehlgeschlagen ist, soll das System keinen erneuten Ladeversuch
  unternehmen, bis der Nutzer den Cleanup-Modus wechselt oder das Modell
  laut Downloader-Status neu verfügbar wird. (`src/pipeline.rs:DictationWorker::handle`,
  Fehler-Cache `cleaner_failed`)
- **REQ-CLEAN-013**: WENN ein zuvor nicht verfügbares Cleanup-Modell
  nachträglich bereitsteht (z. B. Download abgeschlossen), soll das System
  den Fehler-Cache zurücksetzen und den Cleanup ohne Neustart der App
  reaktivieren. (`src/pipeline.rs:DictationWorker::handle`, „Live-Aktivierung")
- **REQ-CLEAN-014**: FALLS ein Nicht-Roh-Modus aktiv ist, aber das
  Cleanup-Modell nicht ladbar war, DANN soll das System den Raw Transcript
  einfügen und dies als Fallback kennzeichnen, statt die Utterance zu
  verwerfen. (`src/pipeline.rs:process_utterance`, `degraded`-Zweig)
- **REQ-CLEAN-015**: Der Cleanup-Aufruf soll nach einer festen Obergrenze von
  15 Sekunden abbrechen, statt die Pipeline unbegrenzt zu blockieren.
  (`src/cleanup.rs:CLEAN_TIMEOUT`)
- **REQ-CLEAN-016**: FALLS das Prompt-Token-Budget von 3000 Token
  überschritten wird, DANN soll das System einen Fehler liefern statt das
  Modell in einen nicht abfangbaren Abbruch laufen zu lassen.
  (`src/cleanup.rs:MAX_PROMPT_TOKENS`, Eval-0001 1600-Wörter-Fall)

## Eigenes Vokabular

- **REQ-CLEAN-017**: Das System soll dem Nutzer erlauben, eine eigene
  Begriffsliste (ein Begriff pro Zeile) in den Einstellungen zu pflegen, die
  in der TOML-Config persistiert wird und ohne App-Neustart auf das nächste
  Diktat wirkt. (`src/config.rs:vocabulary`, Ticket-0012 AK1)
- **REQ-CLEAN-018**: WO das Vokabular nicht leer ist, soll das System es in
  jedem Nicht-Roh-Modus als Korrektur-Liste in die Cleanup-Instruktion
  aufnehmen. (`src/cleanup.rs:vocab_block`, `build_prompt`)
- **REQ-CLEAN-019**: FALLS das Vokabular leer ist, DANN soll das System
  keinen Vokabular-Block in die Instruktion einfügen (Verhalten bleibt
  identisch zum Zustand ohne Vokabular). (`src/cleanup.rs:vocab_block`)
- **REQ-CLEAN-020**: WO die phonetische Vokabular-Korrektur aktiv ist
  (`phonetic_matching`), soll das System Wort-Fenster von 1 bis 3 Wörtern,
  deren Kölner-Phonetik-Code und Anfangsbuchstabe einem Vokabular-Begriff
  entsprechen, vor dem Cleanup durch die exakte Schreibweise des Begriffs
  ersetzen — auch im Modus Roh. (`src/vocab_match.rs:apply`, CONTEXT.md
  „Batch-Modell"/Ticket-0012 Nachtrag)
- **REQ-CLEAN-021**: WO die phonetische Vokabular-Korrektur deaktiviert ist,
  soll das System das Transkript unverändert durchreichen.
  (`src/config.rs:phonetic_matching`, `src/pipeline.rs:process_utterance`)
- **REQ-CLEAN-022**: FALLS ein Vokabular-Begriff nach dem Entfernen von
  Nicht-Alphanumerischem kürzer als 4 Zeichen ist, DANN soll das System ihn
  vom phonetischen Fuzzy-Matching ausschließen, um Kollisionen mit
  gebräuchlichen Wörtern zu vermeiden. (`src/vocab_match.rs:apply`, Ticket-0032
  AK1)
- **REQ-CLEAN-023**: FALLS ein Wort-Fenster denselben Kölner-Phonetik-Code
  wie ein Vokabular-Begriff hat, DANN soll das System die Ersetzung nur
  vornehmen, wenn zusätzlich die Editierdistanz der Wort-Oberflächen
  höchstens 2 beträgt — reine Code-Gleichheit reicht bei kurzen Codes nicht
  aus. (`src/vocab_match.rs:apply`, Ticket-0032 AK1 „Exakt-Kollision")
- **REQ-CLEAN-024**: FALLS mehrere Vokabular-Begriffe mit demselben
  Klang-Code und Anfangsbuchstaben auf dasselbe Wort-Fenster passen, DANN
  soll das System den in der Vokabular-Liste zuerst genannten Begriff
  verwenden. (`src/vocab_match.rs:apply`, Test
  `colliding_terms_first_in_vocab_order_wins`)
- **REQ-CLEAN-025**: Das System soll ein exakt im Transkript vorkommendes
  Vokabular-Wort unverändert lassen und darf es nicht durch sich selbst
  „ersetzen". (`src/vocab_match.rs:apply`, Test `leaves_exact_terms_and_normal_words_alone`)

## Tray-Modus-Schnellwechsel

- **REQ-CLEAN-026**: Das System soll im Tray-Menü einen Eintrag pro
  Cleanup-Modus mit Checkmark-Auswahl anzeigen, wobei genau der aktive Modus
  markiert ist. (`src/tray.rs:Tray::new`, `checked_flags`)
- **REQ-CLEAN-027**: WENN der Nutzer im Tray-Menü einen anderen Modus
  auswählt, soll das System die Config sofort auf diesen Modus setzen; die
  Config bleibt einzige Quelle der Wahrheit, das Tray-Icon zieht per
  `sync_mode` nach. (`src/ui.rs:logic` MenuEvent-Handling, `src/tray.rs:sync_mode`)
- **REQ-CLEAN-028**: Das System soll den aktiven Cleanup-Modus als
  Buchstaben-Badge (R/G/N/L) auf dem Tray-Icon anzeigen — sowohl im Ruhe- als
  auch im Aufnahme-Zustand. (`src/tray.rs:mode_badge`, `mic_icon`)
- **REQ-CLEAN-029**: WÄHREND einer laufenden Aufnahme soll das Tray-Badge den
  für DIESE Utterance aufgelösten Modus zeigen (inkl. Kontext-Awareness-
  Override), nicht zwingend den manuell konfigurierten. (`src/tray.rs:sync_recording`,
  `recording_mode`)
- **REQ-CLEAN-030**: Das System soll jedem der vier Modi ein eindeutiges,
  nicht-leeres Badge-Bitmap zuordnen. (`src/tray.rs:badge_bitmap`, Test
  `mode_badges_are_distinct_and_have_bitmaps`)

## Zahlen-Normalisierung (Natürlich)

- **REQ-CLEAN-031**: WENN der Modus Natürlich aktiv ist, soll das System echte
  Mengen-, Datums- und Zeitangaben — auch kleine, in normale Sätze
  eingebettete Zahlen — von Zahlwort zu Ziffer konvertieren, im selben
  Zahlenformat wie der Modus Geschäftlich (deutsches Tausenderpunkt-Format,
  z. B. „zwölftausendfünfhundert" → „12.500"). Ordinalzahlen („die zweite
  Idee") und feste Redewendungen mit Zahlwörtern („eins zu eins", „Schritt
  für Schritt") bleiben unverändert Wort. (`src/cleanup.rs:mode_instruction`,
  `mode_examples`, PRD-0001, Test
  `cleans_filler_words_without_thinking_leak_and_passes_empty_through`)
