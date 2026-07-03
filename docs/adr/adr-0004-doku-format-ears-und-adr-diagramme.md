# ADR-0004 — Requirements in EARS, Architektur als ADR + Diagramme statt arc42/IREB

- Status: akzeptiert
- Datum: 2026-07-03
- Kontext: siehe `CONTEXT.md` und `docs/adr/`
- Baut auf: keine (Doku-Format-Entscheidung, keine Code-Architektur)

## Kontext und Problemstellung

talker braucht Dokumentation für zwei Dinge: die Systemarchitektur (wie hängen
die Module zusammen, was läuft zur Laufzeit) und fachliche Anforderungen (was
soll das System tun). Der globale Default (persönliche `CLAUDE.md`) sieht dafür
arc42 unter `docs/architecture/` und IREB-Anforderungen unter
`docs/requirements/` vor. Zu klären war, ob dieser Default für ein
Solo-Hobby-Projekt dieser Größe der richtige Aufwand ist, oder ob ein
leichteres Format denselben Nutzen bei weniger Pflegeaufwand liefert.

## Entscheidungstreiber

- Solo-Projekt: kein Team, das von einem vollständigen arc42-Dokument
  (Stakeholder-Kapitel, Risiko-Register etc.) profitiert.
- `docs/adr/` (MADR) und `CONTEXT.md` (Glossar) decken bereits zwei der zwölf
  arc42-Kapitel ab (Architekturentscheidungen, Glossar) — praktische Vorarbeit
  soll nicht dupliziert werden.
- Bestehende Pflicht aus `CLAUDE.md`: „Tests-pro-Story ist Definition of
  Done" — AK↔Test 1:1. Ein Requirements-Format sollte das unterstützen statt
  nur Prosa zu liefern.
- Diagramme sollen im Git-Diff überprüfbar/versionierbar sein, kein
  Bild-Blob.

## Betrachtete Optionen

1. **Volles arc42 (12 Kapitel) + IREB-Fließtext-Requirements.** Globaler
   Default aus der persönlichen `CLAUDE.md`.
2. **Leichte Kombination**: ADR (bereits vorhanden) + kompaktes
   `docs/architecture/overview.md` mit Mermaid-Diagrammen (Bausteinsicht,
   Laufzeitsicht, Kontextsicht), sowie **EARS**-Syntax
   (Ubiquitär/Event-/State-getrieben/Optional/Unwanted-Behavior) für
   Anforderungen unter `docs/requirements/`.
3. **GitHub Spec Kit** (Spec-Driven-Development-Toolkit: `/specify` →
   `/plan` → `/tasks` → `/implement`) als Ersatz für den gesamten
   Doku-Workflow.

## Entscheidung

**Option 2: EARS für Requirements, ADR + leichte Diagramme für Architektur.**
Architektur: `docs/adr/` (Entscheidungen, unverändert) + neu
`docs/architecture/overview.md` mit drei Mermaid-Diagrammen (Bausteinsicht,
Laufzeitsicht/Pipeline, Kontextsicht ggü. macOS-APIs), verlinkt statt
dupliziert gegen `docs/adr/` und `CONTEXT.md`. Requirements:
`docs/requirements/`, Anforderungen als EARS-Sätze statt IREB-Fließtext.

## Begründung

- arc42-Kapitel 9 (Entscheidungen) und das Glossar-Kapitel sind durch
  `docs/adr/` und `CONTEXT.md` bereits abgedeckt; die übrigen zehn Kapitel
  (Stakeholder, Risiko-Register, Qualitätsbaum als eigenes Dokument etc.)
  bringen für ein Solo-Projekt dieser Größe wenig Gegenwert gegenüber dem
  Pflegeaufwand, mehrere weitgehend leere/redundante Kapitel aktuell zu
  halten.
- EARS ist eine Satz-Syntax, keine Dokumentstruktur — sie ersetzt nicht den
  Container `docs/requirements/`, sondern nur den Schreibstil einzelner
  Anforderungen. Der Nutzen: jede EARS-Anforderung („WENN `<Trigger>`, soll
  das System `<Verhalten>`") ist einzeln testbar und mappt fast 1:1 auf einen
  Testfall — deckt sich direkt mit der bestehenden AK↔Test-1:1-Pflicht, ohne
  dass IREB-Fließtext das explizit erzwingen würde.
- Option 3 (GitHub Spec Kit) wurde geprüft, aber verworfen: es ist
  konzeptionell nah an dem, was talker mit `docs/prompt/` + `docs/ticket/`
  bereits informell macht (Prompt/Ticket-Paare pro Feature), arbeitet aber
  auf Feature-Ebene (pro Feature `spec.md`/`plan.md`/`tasks.md`), nicht als
  Gesamt-System-Überblick. Es beantwortet nicht die hier zu klärende Frage
  „wie dokumentieren wir Architektur und Requirements insgesamt", sondern
  wäre allenfalls eine spätere Formalisierung des bestehenden
  Ticket-Workflows — das ist eine andere Entscheidung und bewusst nicht Teil
  dieser ADR.

## Konsequenzen

- **Positiv:** geringerer Pflegeaufwand als volles arc42; Diagramme als
  Mermaid-Text sind im Git-Diff sichtbar und review-fähig statt Bild-Blob;
  EARS-Anforderungen sind direkt testfreundlich, ohne zusätzliche
  Übersetzungsarbeit von Prosa zu Testfällen.
- **Negativ:** bewusste Abweichung vom globalen `CLAUDE.md`-Default
  (arc42 + IREB) — hier dokumentiert, gilt nur für dieses Repo. Kein
  vollständiges Risiko-Register oder Stakeholder-Kapitel; bei wachsendem
  Team oder deutlich steigender Komplexität wäre das nachzuziehen.
- **Verworfen:** volles arc42 + IREB (Overhead für diese Projektgröße),
  GitHub Spec Kit als Ersatz für Architektur-/Requirements-Doku (falscher
  Anwendungsbereich — Feature-Spec-Tool, kein Architektur-/Requirements-Format).
