# ADR-0002 — Menüleisten-App mit egui-Settings statt nativer/Web-UI

- Status: akzeptiert
- Datum: 2026-07-02
- Kontext: siehe `CONTEXT.md`
- Baut auf: ADR-0001 (Rust + macOS-nativ, kein Tauri)

## Kontext und Problemstellung

talker läuft als Hintergrund-Diktier-Assistent. Es braucht: ein Menüleisten-Icon (Status idle/aufnehmend), Aufnahme-Feedback, ein Settings-Fenster (Hotkey, Modell, Cleanup an/aus, Mikrofon) und ein First-Run-Onboarding für die Pflicht-Permissions (Accessibility, Mikrofon). ADR-0001 hat Tauri verworfen und Rust-nativ gewählt — damit ist offen, womit das Settings-Fenster gebaut wird. In reinem Rust ist reichhaltige, nativ aussehende GUI aufwändig.

## Entscheidungstreiber

- Ein-Sprachen-/self-contained-Prinzip aus ADR-0001 (Rust, eine Toolchain).
- Settings ist utilitaristische, selten benutzte UI — nativer Look ist nice-to-have, nicht kritisch.
- Rock-solid + wartbar: möglichst wenig bewegliche Teile, kein IPC-Overhead.

## Betrachtete Optionen

1. **egui** — Immediate-Mode-GUI in reinem Rust.
2. **SwiftUI-Sidecar** — natives Look-and-feel, aber zweite Sprache + Toolchain + IPC.
3. **Tauri** — Web-UI-Shell (ADR-0001 bereits verworfen).
4. **Nur TOML-Config, keine GUI** — minimalst, aber schlechtes Onboarding/UX.

## Entscheidung

**Menüleisten-App (nativ) + Settings-Fenster via egui (reines Rust). Konfiguration persistiert als TOML.** Aufnahme-Feedback über Menüleisten-Icon-Status (optional Sound). Permission-Onboarding minimal nativ.

## Begründung

- egui hält alles bei einer Sprache/Toolchain — konsistent zu ADR-0001, kein IPC, wenig bewegliche Teile.
- Für ein selten geöffnetes Settings-Fenster wiegt nativer Look den Aufwand eines SwiftUI-Sidecars (zweite Sprache, IPC, Build-Komplexität) nicht auf.
- TOML als Persistenz ist einfach, versionierbar, und funktioniert auch headless.

## Konsequenzen

- **Positiv:** eine Toolchain, self-contained, schnell iterierbar; UI-Zustand ↔ TOML klar trennbar.
- **Negativ:** Settings-Fenster sieht nicht 100% macOS-nativ aus (eigener egui-Look, keine System-Widgets). Falls später doch nativer Look gefordert wird → SwiftUI-Sidecar als Folge-ADR, egui-Fenster ersetzbar, da UI-Layer entkoppelt.
- **Verworfen:** SwiftUI-Sidecar (zu viel Komplexität für den Nutzen), Tauri (bereits in ADR-0001 raus), Nur-TOML (Onboarding-UX zu schwach für Permissions).
