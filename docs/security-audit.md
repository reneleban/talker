# Security- & Hygiene-Audit vor Veröffentlichung (Ticket-0020)

Stand: 2026-07-03 · Scope: Repo-Zustand vor Go-Public (Bucket A).

## Reproduzierbare Scan-Kommandos

```sh
# Secret-Scan über die gesamte Git-History
gitleaks git --no-banner --redact .

# Secret-Scan über den Working Tree
gitleaks dir --no-banner --redact .

# Supply-Chain: Advisories (gesamtes Cargo.lock, alle Plattformen)
cargo audit

# Supply-Chain: Advisories + Lizenzen + Quellen, gefiltert auf das
# tatsächliche Build-Target (siehe deny.toml) — das maßgebliche Gate
cargo deny check
```

Tooling: `gitleaks` (Homebrew), `cargo install cargo-audit cargo-deny --locked`.

## Ergebnisse (2026-07-03)

### 1. Secret-Scan

- **History** (65 Commits): keine Funde.
- **Working Tree**: 12 Treffer, alle False Positives — `generic-api-key`-Pattern
  in Build-Artefakten unter `target/` (`libmuda-*.rmeta`). `target/` ist
  untracked und ignoriert, geht nie mit ins Repo.

### 2. .gitignore-Hygiene

- `.DS_Store` aus dem Root entfernt und ignoriert.
- `target/` war bereits ignoriert; Modelldateien (`*.gguf`, `*.onnx`) jetzt
  explizit ignoriert. Keine Modelldateien oder Caches in der History.

### 3. Keine Personen-Pfade

`git grep -E '/Users/[a-z]+'` über alle committeten Dateien: keine Funde.
Die App leitet alle Pfade aus `$HOME` ab (`src/config.rs`, `src/stt.rs`,
`src/cleanup.rs`); `scripts/*.sh` arbeiten relativ zum Repo-Root.

### 4. Egress-Beweis (Privacy-Claim „100 % lokal")

- **Runtime:** Kein HTTP-Client im Dependency-Graph des Binaries. Grep über
  `Cargo.toml`/`Cargo.lock` nach `reqwest`/`ureq`/`hyper`/`curl`/`isahc` u. ä.:
  einziger Treffer ist `ureq` — und der ist ausschließlich eine
  **Build-Dependency** von `sherpa-rs-sys` (Feature `download-binaries`:
  lädt beim *Kompilieren* die vorgebauten sherpa-onnx-/onnxruntime-Dylibs).
  Im ausgelieferten Binary existiert kein Netzwerk-Code. `git grep 'https\?://' src/`: keine Treffer.
- **Modelle:** werden vom Nutzer manuell geladen (Links im README); die App
  selbst lädt nichts nach.
- **Fazit:** kein Phone-Home, keine Telemetrie, kein Egress zur Laufzeit.

### 5. Supply-Chain

- `cargo deny check` (Target `aarch64-apple-darwin`): **advisories ok,
  bans ok, licenses ok, sources ok.** Lizenz-Ausnahmen mit Begründung in
  `deny.toml` (webpki-roots nur Build-Dep; egui-Font-Lizenzen).
- `cargo audit` (prüft das komplette `Cargo.lock`, plattformunabhängig)
  meldet 2 Vulnerabilities (`quick-xml`, RUSTSEC-2026-0194/-0195) und
  mehrere unmaintained-Warnungen (gtk3-Bindings). **Alle liegen in
  Linux/Wayland-only-Zweigen** (via `winit`/`tray-icon`), die auf macOS nie
  kompiliert oder gelinkt werden — dokumentiert akzeptiert. Maßgeblich ist
  das target-gefilterte `cargo deny check`.

### 6. Entitlements / Permissions

`resources/Info.plist` enthält als einzigen Permission-Key
`NSMicrophoneUsageDescription`. Accessibility läuft über den TCC-Prompt zur
Laufzeit (kein Plist-Key nötig). Kein Entitlements-File, keine Sandbox,
keine weiteren Rechte — minimal wie gefordert.

### 7. Autor-Mail in der History

Entscheidung (User, 2026-07-03): `rene@leban.de` bleibt in der History —
kein History-Rewrite. Optional für künftige Commits:
`git config user.email <id>@users.noreply.github.com`.
