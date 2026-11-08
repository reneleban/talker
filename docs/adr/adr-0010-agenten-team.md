# ADR-0010 — Agenten-Team mit eigenen Sessions je Rolle

- Status: akzeptiert; Punkt 2 und die Ghostty-Konsequenzen abgelöst durch ADR-0013
- Datum: 2026-11-06
- Kontext: `AGENTS.md` (Team, Ablauf, Übergabe-Protokoll), `.claude/agents/`, `.claude/skills/`

## Kontext und Problemstellung

talker wird überwiegend von Agenten gebaut. Bisher arbeitete eine Session
alles nacheinander ab; Spec, Entwurf, Umsetzung und Review vermischten sich in
einem Kontext. Ziel: Rollen wie in einer Software-Firma, klare Übergaben, ein
einziges Gate für den Menschen (die Spec), danach automatisch bis zum Draft-PR (seit Ticket 0043 bis zum Merge, siehe Änderungsvermerk bei Punkt 5).
Der Rollenschnitt ist angelehnt an die Lifecycle-Phasen von addyosmani/agent-skills
(Define, Plan, Build, Verify, Review, Ship).

## Entscheidung

1. **11 Rollen** als Agent-Definitionen in `.claude/agents/`: orchestrator,
   product-owner, architect, tech-lead, rust-developer, test-engineer,
   code-reviewer, security-auditor, performance-engineer, merger,
   release-engineer. Modell, Effort, erlaubte und verbotene Tools je Rolle im
   Frontmatter; Urteilsrollen auf Opus, ausführende auf Sonnet.
2. **Eigene Session je Rolle** als Ghostty-Kachel neben dem Orchestrator
   (AppleScript-Split), gestartet und beendet vom Orchestrator (`spawn-worker`),
   Kommunikation per Cross-Session-Messaging; der Auftrag kommt per Nachricht.
   Jede Phase startet eine neue Session mit frischem Kontext.
3. **Worktree je schreibender Rolle** (`../talker-wt/<rolle>`, Branch
   `wt/<rolle>`), stabile Pfade über Tickets hinweg. Ein eigener **merger**
   führt in den Feature-Branch im Integrations-Worktree zusammen; lesende Rollen
   arbeiten dort.
4. **Ticket-Datei als Akte** (`docs/ticket/`, im Haupt-Checkout), Status-Zeile
   und ein Abschnitt je Rolle.
5. **Gates**: Spec-Freigabe durch den Menschen (gilt auch als Push-Freigabe des
   Feature-Branches), Merge nach `main` durch den Menschen, Abnahme (Field-Test)
   vor jedem Release statt je Feature.
   *Änderung 2026-11-08 (Ticket 0043):* Den Merge nach `main` führt der release-engineer unter
   Bedingungen aus (CI grün, verify:fast grün, kein blockierender Befund, kein Konflikt; nicht bei
   Breaking Change, Release, Tag). Spec-Freigabe und Release-Abnahme bleiben Gates des Menschen.
6. **Abläufe eingebettet**: Was eine Rolle braucht, steht in ihrer Definition;
   was mehrere brauchen, ist ein Projekt-Skill im Repo. Keine Verweise auf
   global installierte Skills.

## Betrachtete Optionen

1. **Hauptsession mit Subagents:** günstig, aber keine eigene Kachel je Rolle
   und kein direkter Dialog mit dem PO außerhalb der Hauptsession. Verworfen.
2. **Agent Teams (experimentell, Split-Panes):** Orchestrator als Team-Lead
   öffnet die Panes selbst. Verworfen: Bei Split-Pane-Teammates greifen
   `effort`, `skills` und `disallowedTools` nicht, alle erben den
   Permission-Modus des Leads; Ghostty wird nicht unterstützt.
3. **Eigene Sessions mit Cross-Session-Messaging:** volles Frontmatter je Rolle,
   jede Kachel fragt ihre Permissions selbst. Gewählt.

Für die Skills: Verweise auf globale Skills brechen still, wenn diese fehlen
(unbekannte Frontmatter-Felder werden ignoriert), und sind allgemein statt
auf Rust/macOS zugeschnitten. Daher eingebettet; Upstream-Änderungen kommen
nicht automatisch an.

## Konsequenzen

- Mehr Token-Verbrauch als eine Session; jede Phase lädt den Kontext neu.
- Ein Orchestrator kann eine laufende Session weder leeren noch ihren Agent
  wechseln (Befehle in Nachrichten werden nicht ausgeführt) — nur neu starten.
- Nachrichten zwischen Sessions sind keine Freigabe des Menschen; der
  Orchestrator prüft den Status in der Ticket-Datei.
- Alle Rollen laufen im Permission-Modus `auto` (`spawn.sh` setzt ihn); das
  Frontmatter-Feld `permissionMode` greift bei `--agent`-Sessions nicht.
  Die Grenzen je Rolle ziehen `tools`/`disallowedTools`.
- Jeder Worktree hat ein eigenes `target/`; der erste Build je Rolle dauert lange.
- Neue Worktree-Pfade fragen einmal nach Ordner-Vertrauen.
- Der Orchestrator muss in Ghostty laufen; macOS fragt einmal nach der
  Automation-Berechtigung zur Steuerung von Ghostty.
- Neue Sessions erben sonst die `CLAUDE*`-Umgebung des Orchestrators und gelten
  als Kind-Session ohne Posteingang; `spawn.sh` entfernt sie.
- Ein Start-Prompt wird in einer Kachel ohne Fokus nicht abgeschickt; deshalb
  kommt der Auftrag per Nachricht.
- `docs/ticket/` ist gitignored; die Akte existiert nur im Haupt-Checkout und
  wird jeder Rolle per `--add-dir` zugänglich gemacht.
