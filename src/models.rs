//! models: Modell-Downloader-Kern (Ticket-0028) — Präsenz-/Integritäts-Check,
//! Download hinter mockbarem Trait, State-Maschine je Modell, Live-Aktivierung.
//!
//! Kein Download ohne Consent (Lizenz-Zustimmung, Flag in der Config).
//! Parakeet lädt der Aufrufer blockierend (STT = Kern), gemma läuft als
//! Hintergrund-Task; wird gemma `ready`, schaltet der geteilte `ModelsState`
//! die Nicht-Roh-Modi ohne Neustart frei. Die UI dazu liefert Ticket-0029.

use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::cleanup::CleanupMode;
use crate::error::{Result, TalkerError};

/// Parakeet TDT 0.6b v3 int8 — nicht-gated sherpa-onnx-Release (keine HF-Auth).
pub const PARAKEET_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";
/// SHA-256 des Release-Tarballs (upstream unpubliziert — einmal geladen,
/// berechnet und gepinnt, 2026-07-03).
pub const PARAKEET_ARCHIVE_SHA256: &str =
    "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf";
/// Verzeichnisname im Tarball = erwarteter `stt_model_dir`-Name.
pub const PARAKEET_DIR_NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
/// Die vier Modell-Dateien mit gepinnten SHA-256 (aus dem verifizierten
/// Tarball berechnet) — Basis des Integritäts-Checks beim Laden.
pub const PARAKEET_FILES: [(&str, &str); 4] = [
    (
        "encoder.int8.onnx",
        "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
    ),
    (
        "decoder.int8.onnx",
        "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
    ),
    (
        "joiner.int8.onnx",
        "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
    ),
    (
        "tokens.txt",
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
];

/// gemma4:e2b GGUF Q8_0 — public HF-resolve-URL von ggml-org (ADR-0003).
pub const GEMMA_URL: &str =
    "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q8_0.gguf";
pub const GEMMA_FILE_NAME: &str = "gemma-4-E2B-it-Q8_0.gguf";
/// SHA-256 des GGUF (Grill 2026-07-03; deckt sich mit dem HF-ETag).
pub const GEMMA_SHA256: &str = "e049411c01fb7a81161768c52e38828970e55a64e22738957adcbe51d20f1c8e";

/// Beide Modelle des Features. Parakeet = Pflicht/blockierend, gemma = Hintergrund.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelId {
    Parakeet,
    Gemma,
}

/// Ergebnis des Präsenz-/Integritäts-Checks (AK 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    Ready,
    Missing,
    Corrupt,
}

/// Zustand eines Modells in der Download-Maschine (AK 3):
/// missing → consent-pending → downloading(%) → verifying → ready | error.
/// `Corrupt` = Datei da, aber Hash falsch (AK 5) — Retry möglich.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Missing,
    ConsentPending,
    Downloading { pct: u8 },
    Verifying,
    Ready,
    Corrupt,
    Error(String),
}

impl ModelState {
    /// Statuszeile eines Modells im „Modelle"-Bereich: (Text, Aktions-Button?).
    /// Button-Beschriftung: „Neu laden" (fehlt) bzw. „Reparieren" (kaputt/Fehler).
    pub fn status_line(&self) -> (String, Option<&'static str>) {
        match self {
            ModelState::Ready => ("installiert ✓".into(), None),
            ModelState::Missing => ("fehlt".into(), Some("Neu laden")),
            ModelState::ConsentPending => ("wartet auf Lizenz-Zustimmung".into(), None),
            ModelState::Downloading { pct } => (format!("lädt … {pct} %"), None),
            ModelState::Verifying => ("wird geprüft …".into(), None),
            ModelState::Corrupt => ("beschädigt (Checksum)".into(), Some("Reparieren")),
            ModelState::Error(e) => (format!("Fehler: {e}"), Some("Reparieren")),
        }
    }

    /// Fortschritts-Anteil (0–1) fürs Balken-UI eines Modells; None = kein Balken.
    pub fn progress_fraction(&self) -> Option<f32> {
        match self {
            ModelState::Downloading { pct } => Some(f32::from(*pct) / 100.0),
            ModelState::Verifying => Some(1.0),
            _ => None,
        }
    }

    /// Hinweistext bei PTT-Druck, solange das Modell nicht ready ist (AK 3).
    /// None = PTT frei.
    pub fn setup_hint(&self) -> Option<String> {
        match self {
            ModelState::Ready => None,
            ModelState::Downloading { pct } => Some(format!("talker richtet sich ein … {pct} %")),
            ModelState::Verifying => Some("talker richtet sich ein … Modell wird geprüft".into()),
            ModelState::ConsentPending | ModelState::Missing => {
                Some("Einrichtung nötig — Einstellungen öffnen".into())
            }
            ModelState::Corrupt | ModelState::Error(_) => {
                Some("Modell-Download fehlgeschlagen — Einstellungen öffnen".into())
            }
        }
    }
}

/// Was für ein Modell installiert wird — parametrisierbar, damit Tests ohne
/// die echten (gepinnten) Konstanten arbeiten können.
#[derive(Debug, Clone)]
pub enum ModelSpec {
    /// Einzelne Datei (gemma): laden → verifizieren → an den Zielort verschieben.
    File {
        url: String,
        sha256: String,
        dest: PathBuf,
    },
    /// tar.bz2-Archiv (Parakeet): laden → verifizieren → nach `root` entpacken;
    /// `files` = erwartete Dateien mit gepinnten Hashes (Post-Extract-Check).
    Archive {
        url: String,
        sha256: String,
        root: PathBuf,
        files: Vec<(PathBuf, String)>,
    },
}

/// Default-Wurzel der Modell-Ablage: ~/Library/Application Support/talker/models.
pub fn default_models_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/talker/models")
}

/// Spec eines Modells unterhalb von `models_root` (Produktions-Konstanten).
pub fn spec_for(id: ModelId, models_root: &Path) -> ModelSpec {
    match id {
        ModelId::Parakeet => ModelSpec::Archive {
            url: PARAKEET_URL.into(),
            sha256: PARAKEET_ARCHIVE_SHA256.into(),
            root: models_root.to_path_buf(),
            files: PARAKEET_FILES
                .iter()
                .map(|(name, sha)| {
                    (
                        models_root.join(PARAKEET_DIR_NAME).join(name),
                        (*sha).to_string(),
                    )
                })
                .collect(),
        },
        ModelId::Gemma => ModelSpec::File {
            url: GEMMA_URL.into(),
            sha256: GEMMA_SHA256.into(),
            dest: models_root.join(GEMMA_FILE_NAME),
        },
    }
}

impl ModelSpec {
    fn url(&self) -> &str {
        match self {
            ModelSpec::File { url, .. } | ModelSpec::Archive { url, .. } => url,
        }
    }

    fn sha256(&self) -> &str {
        match self {
            ModelSpec::File { sha256, .. } | ModelSpec::Archive { sha256, .. } => sha256,
        }
    }

    /// Ablageort des laufenden (Teil-)Downloads — bleibt bei Abbruch liegen
    /// und wird beim nächsten Versuch fortgesetzt (Ticket-0031).
    fn partial_path(&self) -> PathBuf {
        match self {
            ModelSpec::File { dest, .. } => {
                let mut name = dest.file_name().unwrap_or_default().to_os_string();
                name.push(".partial");
                dest.with_file_name(name)
            }
            ModelSpec::Archive { root, .. } => root.join("model-archive.partial"),
        }
    }

    /// Erwartete Dateien + Hashes der fertigen Installation.
    fn installed_files(&self) -> Vec<(PathBuf, String)> {
        match self {
            ModelSpec::File { dest, sha256, .. } => vec![(dest.clone(), sha256.clone())],
            ModelSpec::Archive { files, .. } => files.clone(),
        }
    }
}

/// SHA-256 (hex, lowercase) einer Datei — streamend, kein Voll-Einlesen.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .map_err(|e| TalkerError::Download(format!("{} nicht lesbar: {e}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| TalkerError::Download(format!("{} nicht lesbar: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Präsenz-/Integritäts-Check (AK 1): alle Dateien da + Hashes ok → Ready;
/// keine Datei da → Missing; teilweise da oder ein Hash falsch → Corrupt.
pub fn check_files(files: &[(PathBuf, String)]) -> ModelStatus {
    let present: Vec<bool> = files.iter().map(|(p, _)| p.is_file()).collect();
    if present.iter().all(|p| !p) {
        return ModelStatus::Missing;
    }
    if present.iter().any(|p| !p) {
        return ModelStatus::Corrupt;
    }
    for (path, expected) in files {
        match sha256_file(path) {
            Ok(actual) if actual == *expected => {}
            _ => return ModelStatus::Corrupt,
        }
    }
    ModelStatus::Ready
}

/// Check einer Modell-Installation laut Spec.
pub fn check(spec: &ModelSpec) -> ModelStatus {
    check_files(&spec.installed_files())
}

/// Reiner Präsenz-Check ohne Hashing — für den App-Start: die vollen
/// Checksummen (gemma ~5 GB) würden den Start Sekunden blockieren; sie
/// prüft `spawn_integrity_check` im Hintergrund nach.
pub fn check_presence(spec: &ModelSpec) -> ModelStatus {
    let files = spec.installed_files();
    let present: Vec<bool> = files.iter().map(|(p, _)| p.is_file()).collect();
    if present.iter().all(|p| !p) {
        return ModelStatus::Missing;
    }
    if present.iter().any(|p| !p) {
        return ModelStatus::Corrupt;
    }
    ModelStatus::Ready
}

/// Anfangszustand der Maschine aus Disk-Status + Consent-Flag.
pub fn initial_state(status: ModelStatus, consent: bool) -> ModelState {
    match status {
        ModelStatus::Ready => ModelState::Ready,
        ModelStatus::Corrupt => ModelState::Corrupt,
        ModelStatus::Missing if consent => ModelState::Missing,
        ModelStatus::Missing => ModelState::ConsentPending,
    }
}

/// Download-Fortschritt in Prozent; unbekannte Gesamtgröße → 0.
pub fn percent(done: u64, total: Option<u64>) -> u8 {
    match total {
        Some(t) if t > 0 => ((done.saturating_mul(100)) / t).min(100) as u8,
        _ => 0,
    }
}

/// Lädt `url` nach `dest` und meldet Fortschritt als (geladene Bytes,
/// Gesamtgröße falls bekannt). Mockbar (AK 2). Resume-Vertrag (Ticket-0031):
/// existiert `dest` bereits, wird ab dessen Ende fortgesetzt — vorhandene
/// Bytes zählen in den Fortschritt ein.
pub trait ModelFetcher: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()>;
}

/// Produktions-Fetcher über HTTPS (ureq, folgt Redirects). Teildownloads
/// werden per `Range`-Request fortgesetzt (206 → anhängen); ignoriert der
/// Server Range (200), wird von vorn geschrieben. 416 = Teildownload
/// ungültig → löschen, der nächste Versuch startet frisch.
pub struct HttpFetcher;

impl ModelFetcher for HttpFetcher {
    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        let map =
            |what: &str, e: &dyn std::fmt::Display| TalkerError::Download(format!("{what}: {e}"));
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|e| map("Modell-Verzeichnis", &e))?;
        }
        let offset = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        let mut req = ureq::get(url);
        if offset > 0 {
            req = req.header("Range", format!("bytes={offset}-"));
        }
        let mut resp = match req.call() {
            Ok(resp) => resp,
            Err(ureq::Error::StatusCode(416)) => {
                let _ = fs::remove_file(dest);
                return Err(TalkerError::Download(
                    "Teildownload ungültig (HTTP 416) — der nächste Versuch startet neu".into(),
                ));
            }
            Err(e) => return Err(map(&format!("Download {url}"), &e)),
        };
        // 206 = Server setzt ab Offset fort; alles andere (200) liefert den
        // kompletten Body → Datei von vorn schreiben.
        let resumed = offset > 0 && resp.status() == 206;
        let start = if resumed { offset } else { 0 };
        let total = resp.body().content_length().map(|len| start + len);
        let mut reader = resp.body_mut().as_reader();
        let file = if resumed {
            fs::OpenOptions::new().append(true).open(dest)
        } else {
            fs::File::create(dest)
        }
        .map_err(|e| map(&format!("{} nicht schreibbar", dest.display()), &e))?;
        let mut writer = BufWriter::new(file);
        let mut buf = [0u8; 64 * 1024];
        let mut done: u64 = start;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| map("Download abgebrochen", &e))?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buf[..n])
                .map_err(|e| map(&format!("{} nicht schreibbar", dest.display()), &e))?;
            done += n as u64;
            progress(done, total);
        }
        writer
            .flush()
            .map_err(|e| map(&format!("{} nicht schreibbar", dest.display()), &e))?;
        Ok(())
    }
}

/// Geteilter Zustand beider Modelle — Kern der Live-Aktivierung (AK 4):
/// Pipeline/UI lesen hier, der Download-Runner schreibt.
pub struct ModelsState {
    parakeet: Mutex<ModelState>,
    gemma: Mutex<ModelState>,
}

impl ModelsState {
    pub fn new(parakeet: ModelState, gemma: ModelState) -> Self {
        Self {
            parakeet: Mutex::new(parakeet),
            gemma: Mutex::new(gemma),
        }
    }

    /// Zustand aus dem Disk-Befund unter `models_root` + Consent-Flag.
    pub fn from_disk(models_root: &Path, consent: bool) -> Self {
        let state = |id| initial_state(check(&spec_for(id, models_root)), consent);
        Self::new(state(ModelId::Parakeet), state(ModelId::Gemma))
    }

    /// Wie `from_disk`, aber nur Präsenz (kein Hashing) — Start-Pfad;
    /// danach `spawn_integrity_check` fürs volle Checksum-Urteil starten.
    pub fn from_disk_quick(models_root: &Path, consent: bool) -> Self {
        let state = |id| initial_state(check_presence(&spec_for(id, models_root)), consent);
        Self::new(state(ModelId::Parakeet), state(ModelId::Gemma))
    }

    fn slot(&self, id: ModelId) -> &Mutex<ModelState> {
        match id {
            ModelId::Parakeet => &self.parakeet,
            ModelId::Gemma => &self.gemma,
        }
    }

    pub fn get(&self, id: ModelId) -> ModelState {
        self.slot(id)
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    pub fn set(&self, id: ModelId, state: ModelState) {
        match self.slot(id).lock() {
            Ok(mut s) => *s = state,
            Err(e) => *e.into_inner() = state,
        }
    }

    /// PTT/STT nutzbar? Nur wenn Parakeet installiert und verifiziert ist.
    pub fn stt_ready(&self) -> bool {
        self.get(ModelId::Parakeet) == ModelState::Ready
    }

    /// Läuft gerade ein Download/Verify? (Ticket-0030: Self-Relaunch wartet,
    /// statt einen laufenden Modell-Download abzubrechen.)
    pub fn any_download_running(&self) -> bool {
        [ModelId::Parakeet, ModelId::Gemma].into_iter().any(|id| {
            matches!(
                self.get(id),
                ModelState::Downloading { .. } | ModelState::Verifying
            )
        })
    }

    /// Live-Aktivierung: Nicht-Roh-Modi sind verfügbar, sobald gemma ready ist.
    pub fn llm_modes_available(&self) -> bool {
        self.get(ModelId::Gemma) == ModelState::Ready
    }

    /// Ist `mode` aktuell wählbar? `Roh` immer, LLM-Modi erst mit gemma.
    pub fn mode_available(&self, mode: CleanupMode) -> bool {
        !mode.uses_llm() || self.llm_modes_available()
    }
}

/// Führt einen Download durch (blockierend) und pflegt die State-Maschine:
/// downloading(%) → verifying → ready; Fehler → error, Hash-Mismatch → corrupt
/// (Datei gelöscht). Retry = erneut aufrufen — liegen gebliebene Teildownloads
/// werden fortgesetzt (Ticket-0031), nur Hash-Mismatch räumt auf (AK 5).
/// Ohne Consent passiert kein Fetch (consent-pending).
pub fn run_download(
    state: &ModelsState,
    id: ModelId,
    spec: &ModelSpec,
    fetcher: &dyn ModelFetcher,
    consent: bool,
) -> Result<()> {
    if !consent {
        state.set(id, ModelState::ConsentPending);
        return Err(TalkerError::Download(
            "Download ohne Lizenz-Zustimmung verweigert".into(),
        ));
    }
    let partial = spec.partial_path();

    state.set(id, ModelState::Downloading { pct: 0 });
    let mut last_pct = 0u8;
    let fetched = fetcher.fetch(spec.url(), &partial, &mut |done, total| {
        let pct = percent(done, total);
        if pct != last_pct {
            last_pct = pct;
            state.set(id, ModelState::Downloading { pct });
        }
    });
    if let Err(e) = fetched {
        // Teildownload bewusst liegen lassen — der Retry setzt fort.
        state.set(id, ModelState::Error(e.to_string()));
        return Err(e);
    }

    state.set(id, ModelState::Verifying);
    let actual = match sha256_file(&partial) {
        Ok(h) => h,
        Err(e) => {
            let _ = fs::remove_file(&partial);
            state.set(id, ModelState::Error(e.to_string()));
            return Err(e);
        }
    };
    if actual != spec.sha256() {
        let _ = fs::remove_file(&partial);
        state.set(id, ModelState::Corrupt);
        return Err(TalkerError::Download(format!(
            "Checksum-Fehler: erwartet {}, berechnet {actual} — Download verworfen",
            spec.sha256()
        )));
    }

    if let Err(e) = finalize(spec, &partial) {
        let _ = fs::remove_file(&partial);
        state.set(id, ModelState::Error(e.to_string()));
        return Err(e);
    }
    // Archiv: entpackte Dateien gegen die gepinnten Hashes prüfen — fängt
    // falsche Pins und kaputte Extraktion, bevor irgendwer `ready` glaubt.
    if matches!(spec, ModelSpec::Archive { .. }) && check(spec) != ModelStatus::Ready {
        state.set(id, ModelState::Corrupt);
        return Err(TalkerError::Download(
            "Entpackte Modell-Dateien bestehen den Integritäts-Check nicht".into(),
        ));
    }
    state.set(id, ModelState::Ready);
    Ok(())
}

/// Verifizierten Download an seinen Platz bringen: Datei umbenennen bzw.
/// Archiv entpacken (tar schützt gegen Pfad-Ausbrüche) und Tarball entfernen.
fn finalize(spec: &ModelSpec, partial: &Path) -> Result<()> {
    let map = |what: &str, e: &dyn std::fmt::Display| TalkerError::Download(format!("{what}: {e}"));
    match spec {
        ModelSpec::File { dest, .. } => fs::rename(partial, dest)
            .map_err(|e| map(&format!("{} nicht schreibbar", dest.display()), &e)),
        ModelSpec::Archive { root, .. } => {
            let file = fs::File::open(partial).map_err(|e| map("Archiv nicht lesbar", &e))?;
            let decoder = bzip2::read::BzDecoder::new(BufReader::new(file));
            tar::Archive::new(decoder)
                .unpack(root)
                .map_err(|e| map("Archiv entpacken", &e))?;
            let _ = fs::remove_file(partial);
            Ok(())
        }
    }
}

/// Startet einen Download als Hintergrund-Task (gemma, AK 3); das Ergebnis
/// trägt der geteilte `ModelsState`, der Fehlerpfad steht dort als `Error`.
pub fn spawn_background_download(
    state: Arc<ModelsState>,
    id: ModelId,
    spec: ModelSpec,
    fetcher: Arc<dyn ModelFetcher>,
    consent: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = run_download(&state, id, &spec, fetcher.as_ref(), consent) {
            eprintln!("talker: Modell-Download ({id:?}) fehlgeschlagen — {e}");
        }
    })
}

/// Braucht dieses Modell (jetzt) einen Download-Anstoß? Laufende Downloads,
/// wartender Consent und fertige Modelle brauchen keinen.
pub fn needs_download(state: &ModelState) -> bool {
    matches!(
        state,
        ModelState::Missing | ModelState::Corrupt | ModelState::Error(_)
    )
}

/// Stößt den Download EINES Modells an (Hintergrund-Thread, HttpFetcher),
/// wenn es einen braucht. Setzt den Zustand synchron auf downloading —
/// Doppelstart-Schutz gegen schnelle Wiederholungs-Klicks.
pub fn start_download(state: &Arc<ModelsState>, id: ModelId, models_root: &Path, consent: bool) {
    if !consent || !needs_download(&state.get(id)) {
        return;
    }
    state.set(id, ModelState::Downloading { pct: 0 });
    spawn_background_download(
        Arc::clone(state),
        id,
        spec_for(id, models_root),
        Arc::new(HttpFetcher),
        consent,
    );
}

/// Stößt alle nötigen Downloads an (Erst-Start nach Consent bzw. App-Start
/// mit unvollständigem Setup). Ohne Consent passiert nichts.
pub fn start_needed_downloads(state: &Arc<ModelsState>, models_root: &Path, consent: bool) {
    for id in [ModelId::Parakeet, ModelId::Gemma] {
        start_download(state, id, models_root, consent);
    }
}

/// Voller Checksum-Check im Hintergrund (Start-Pfad nach `from_disk_quick`):
/// stuft präsenz-`Ready` geglaubte Modelle bei Hash-Mismatch auf `Corrupt`
/// zurück — UI/PTT-Gating reagieren über den geteilten State.
pub fn spawn_integrity_check(
    state: Arc<ModelsState>,
    models_root: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for id in [ModelId::Parakeet, ModelId::Gemma] {
            if state.get(id) != ModelState::Ready {
                continue;
            }
            let status = check(&spec_for(id, &models_root));
            // Nur zurückstufen, wenn zwischenzeitlich nichts anderes lief.
            if status != ModelStatus::Ready && state.get(id) == ModelState::Ready {
                eprintln!("talker: Modell {id:?} besteht den Checksum-Check nicht — {status:?}.");
                state.set(id, ModelState::Corrupt);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Status→Darstellung-Mapping (Ticket-0029, DoD).
    #[test]
    fn status_line_maps_every_state() {
        let cases: [(ModelState, &str, Option<&str>); 7] = [
            (ModelState::Ready, "installiert ✓", None),
            (ModelState::Missing, "fehlt", Some("Neu laden")),
            (
                ModelState::ConsentPending,
                "wartet auf Lizenz-Zustimmung",
                None,
            ),
            (ModelState::Downloading { pct: 7 }, "lädt … 7 %", None),
            (ModelState::Verifying, "wird geprüft …", None),
            (
                ModelState::Corrupt,
                "beschädigt (Checksum)",
                Some("Reparieren"),
            ),
            (
                ModelState::Error("Netz weg".into()),
                "Fehler: Netz weg",
                Some("Reparieren"),
            ),
        ];
        for (state, expected_text, expected_action) in cases {
            let (text, action) = state.status_line();
            assert_eq!(text, expected_text, "{state:?}");
            assert_eq!(action, expected_action, "{state:?}");
        }
    }

    #[test]
    fn progress_fraction_only_for_running_downloads() {
        assert_eq!(
            ModelState::Downloading { pct: 50 }.progress_fraction(),
            Some(0.5)
        );
        assert_eq!(
            ModelState::Downloading { pct: 0 }.progress_fraction(),
            Some(0.0)
        );
        assert_eq!(ModelState::Verifying.progress_fraction(), Some(1.0));
        assert_eq!(ModelState::Ready.progress_fraction(), None);
        assert_eq!(ModelState::Missing.progress_fraction(), None);
    }

    /// PTT-Hinweis (AK 3): „richtet sich ein … X %" während des Downloads,
    /// None sobald das Modell ready ist.
    #[test]
    fn setup_hint_reports_progress_and_clears_when_ready() {
        assert_eq!(ModelState::Ready.setup_hint(), None);
        assert_eq!(
            ModelState::Downloading { pct: 42 }.setup_hint().unwrap(),
            "talker richtet sich ein … 42 %"
        );
        assert!(
            ModelState::Verifying
                .setup_hint()
                .unwrap()
                .contains("geprüft")
        );
        for state in [ModelState::ConsentPending, ModelState::Missing] {
            assert!(
                state.setup_hint().unwrap().contains("Einstellungen"),
                "{state:?}"
            );
        }
        for state in [ModelState::Corrupt, ModelState::Error("x".into())] {
            assert!(
                state.setup_hint().unwrap().contains("fehlgeschlagen"),
                "{state:?}"
            );
        }
    }

    /// Frisches, test-eigenes Verzeichnis unterm System-Tempdir (wie config.rs).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("talker-models-test-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Schritte des skriptbaren Fakes — mit derselben Resume-Semantik wie der
    /// echte Fetcher (Ticket-0031): vorhandene Bytes in `dest` bleiben stehen,
    /// Geliefertes wird angehängt.
    enum FakeStep {
        /// Bytes anhängen, Erfolg.
        Deliver(Vec<u8>),
        /// Bytes anhängen, dann Fehler (Abbruch mitten im Download).
        AbortAfter(Vec<u8>, String),
        /// Sofortiger Fehler, nichts geschrieben.
        Fail(String),
    }

    struct FakeFetcher {
        calls: AtomicUsize,
        script: Mutex<VecDeque<FakeStep>>,
        /// Beim jeweiligen Aufruf vorgefundene `dest`-Größe (Resume-Nachweis).
        seen_offsets: Mutex<Vec<u64>>,
    }

    impl FakeFetcher {
        fn new(script: Vec<FakeStep>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                script: Mutex::new(script.into()),
                seen_offsets: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn seen_offsets(&self) -> Vec<u64> {
            self.seen_offsets.lock().unwrap().clone()
        }
    }

    impl ModelFetcher for FakeFetcher {
        fn fetch(
            &self,
            _url: &str,
            dest: &Path,
            progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let offset = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            self.seen_offsets.lock().unwrap().push(offset);
            let append = |bytes: &[u8]| {
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dest)
                    .unwrap();
                f.write_all(bytes).unwrap();
            };
            let step = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("unerwarteter fetch-Aufruf");
            match step {
                FakeStep::Deliver(bytes) => {
                    let total = offset + bytes.len() as u64;
                    progress(offset + bytes.len() as u64 / 2, Some(total));
                    append(&bytes);
                    progress(total, Some(total));
                    Ok(())
                }
                FakeStep::AbortAfter(bytes, msg) => {
                    append(&bytes);
                    progress(offset + bytes.len() as u64, None);
                    Err(TalkerError::Download(msg))
                }
                FakeStep::Fail(msg) => Err(TalkerError::Download(msg)),
            }
        }
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn file_spec(dir: &TempDir, content: &[u8]) -> ModelSpec {
        ModelSpec::File {
            url: "https://example.invalid/modell.gguf".into(),
            sha256: sha_of(content),
            dest: dir.path("modell.gguf"),
        }
    }

    /// tar.bz2 mit Verzeichnis `m/` und den gegebenen Dateien bauen.
    fn build_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("m/{name}"), *content)
                .unwrap();
        }
        let tar_bytes = tar.into_inner().unwrap();
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    fn archive_spec(dir: &TempDir, archive: &[u8], files: &[(&str, &[u8])]) -> ModelSpec {
        ModelSpec::Archive {
            url: "https://example.invalid/modell.tar.bz2".into(),
            sha256: sha_of(archive),
            root: dir.0.clone(),
            files: files
                .iter()
                .map(|(name, content)| (dir.path(&format!("m/{name}")), sha_of(content)))
                .collect(),
        }
    }

    // --- AK 1: Präsenz-/Integritäts-Check -----------------------------------

    #[test]
    fn sha256_file_matches_known_test_vector() {
        // NIST-Vektor: sha256("abc").
        let dir = TempDir::new("vector");
        let p = dir.path("abc.txt");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn check_reports_ready_missing_and_corrupt() {
        let dir = TempDir::new("check");
        let good = dir.path("good.bin");
        fs::write(&good, b"inhalt").unwrap();
        let sha = sha_of(b"inhalt");

        // Alle da + Hash ok → Ready.
        assert_eq!(
            check_files(&[(good.clone(), sha.clone())]),
            ModelStatus::Ready
        );
        // Gar nichts da → Missing.
        assert_eq!(
            check_files(&[(dir.path("fehlt.bin"), sha.clone())]),
            ModelStatus::Missing
        );
        // Datei da, Hash falsch → Corrupt.
        assert_eq!(
            check_files(&[(good.clone(), sha_of(b"anderer inhalt"))]),
            ModelStatus::Corrupt
        );
        // Teilweise da (Parakeet: 3 von 4 Dateien) → Corrupt, nicht Missing.
        assert_eq!(
            check_files(&[(good, sha.clone()), (dir.path("fehlt.bin"), sha)]),
            ModelStatus::Corrupt
        );
    }

    #[test]
    fn check_edge_cases_empty_file_and_empty_list() {
        let dir = TempDir::new("check-edge");
        // Grenzfall leere Datei: zählt als vorhanden, Hash entscheidet.
        let empty = dir.path("leer.bin");
        fs::write(&empty, b"").unwrap();
        assert_eq!(
            check_files(&[(empty.clone(), sha_of(b""))]),
            ModelStatus::Ready
        );
        assert_eq!(check_files(&[(empty, sha_of(b"x"))]), ModelStatus::Corrupt);
        // Leere Liste: nichts erwartet = nichts da → Missing (dokumentiert).
        assert_eq!(check_files(&[]), ModelStatus::Missing);
    }

    #[test]
    fn spec_for_builds_paths_under_models_root() {
        let root = Path::new("/tmp/wurzel");
        let ModelSpec::Archive { files, .. } = spec_for(ModelId::Parakeet, root) else {
            panic!("Parakeet muss ein Archiv sein");
        };
        assert_eq!(files.len(), 4);
        assert!(
            files[0]
                .0
                .starts_with("/tmp/wurzel/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8")
        );
        let ModelSpec::File { dest, sha256, .. } = spec_for(ModelId::Gemma, root) else {
            panic!("gemma muss eine Einzeldatei sein");
        };
        assert_eq!(dest, root.join(GEMMA_FILE_NAME));
        assert_eq!(sha256, GEMMA_SHA256);
    }

    // --- AK 3: State-Maschine ------------------------------------------------

    #[test]
    fn initial_state_maps_disk_status_and_consent() {
        assert_eq!(
            initial_state(ModelStatus::Ready, false),
            ModelState::Ready,
            "Ready braucht keinen Consent (Datei ist schon da)"
        );
        assert_eq!(
            initial_state(ModelStatus::Missing, false),
            ModelState::ConsentPending
        );
        assert_eq!(
            initial_state(ModelStatus::Missing, true),
            ModelState::Missing
        );
        assert_eq!(
            initial_state(ModelStatus::Corrupt, true),
            ModelState::Corrupt
        );
    }

    #[test]
    fn percent_handles_bounds_and_unknown_total() {
        assert_eq!(percent(0, Some(100)), 0);
        assert_eq!(percent(50, Some(100)), 50);
        assert_eq!(percent(100, Some(100)), 100);
        assert_eq!(percent(150, Some(100)), 100, "über total → gedeckelt");
        assert_eq!(percent(1, None), 0, "unbekannte Größe → 0");
        assert_eq!(percent(1, Some(0)), 0, "total 0 → keine Division");
    }

    // --- AK 2/3/5: Download, Fehler, Checksum, Retry -------------------------

    #[test]
    fn successful_file_download_ends_ready_with_file_in_place() {
        let dir = TempDir::new("file-ok");
        let spec = file_spec(&dir, b"gguf-bytes");
        let fetcher = FakeFetcher::new(vec![FakeStep::Deliver(b"gguf-bytes".to_vec())]);
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        run_download(&state, ModelId::Gemma, &spec, &fetcher, true).unwrap();

        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready);
        assert_eq!(fs::read(dir.path("modell.gguf")).unwrap(), b"gguf-bytes");
        assert!(!spec.partial_path().exists(), "Partial muss weg sein");
        assert_eq!(check(&spec), ModelStatus::Ready);
    }

    #[test]
    fn no_download_without_consent() {
        let dir = TempDir::new("consent");
        let spec = file_spec(&dir, b"x");
        let fetcher = FakeFetcher::new(vec![FakeStep::Deliver(b"x".to_vec())]);
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        let err = run_download(&state, ModelId::Gemma, &spec, &fetcher, false).unwrap_err();

        assert_eq!(fetcher.calls(), 0, "ohne Consent darf kein Fetch passieren");
        assert_eq!(state.get(ModelId::Gemma), ModelState::ConsentPending);
        assert!(err.to_string().contains("Zustimmung"));
    }

    #[test]
    fn fetch_error_sets_error_state_and_retry_recovers() {
        let dir = TempDir::new("retry");
        let spec = file_spec(&dir, b"inhalt");
        let fetcher = FakeFetcher::new(vec![
            FakeStep::Fail("Netz weg".into()),
            FakeStep::Deliver(b"inhalt".to_vec()),
        ]);
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        // 1. Versuch: Fehler → error-State, kein Partial-Rest.
        assert!(run_download(&state, ModelId::Gemma, &spec, &fetcher, true).is_err());
        assert!(matches!(state.get(ModelId::Gemma), ModelState::Error(_)));
        assert!(!spec.partial_path().exists());

        // 2. Versuch (Retry) = erneuter Aufruf → Ready.
        run_download(&state, ModelId::Gemma, &spec, &fetcher, true).unwrap();
        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready);
        assert_eq!(fetcher.calls(), 2);
    }

    #[test]
    fn hash_mismatch_deletes_download_sets_corrupt_and_retry_recovers() {
        let dir = TempDir::new("mismatch");
        let spec = file_spec(&dir, b"erwarteter inhalt");
        let fetcher = FakeFetcher::new(vec![
            FakeStep::Deliver(b"manipulierter inhalt".to_vec()),
            FakeStep::Deliver(b"erwarteter inhalt".to_vec()),
        ]);
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        let err = run_download(&state, ModelId::Gemma, &spec, &fetcher, true).unwrap_err();

        assert!(err.to_string().contains("Checksum"));
        assert_eq!(state.get(ModelId::Gemma), ModelState::Corrupt);
        assert!(
            !spec.partial_path().exists(),
            "Mismatch-Datei muss weg sein"
        );
        assert!(!dir.path("modell.gguf").exists(), "nichts installiert");

        run_download(&state, ModelId::Gemma, &spec, &fetcher, true).unwrap();
        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready);
    }

    /// Resume (Ticket-0031): Abbruch mitten im Download lässt den Teildownload
    /// liegen, der Retry setzt ab dessen Größe fort statt neu zu laden.
    #[test]
    fn interrupted_download_resumes_on_retry() {
        let dir = TempDir::new("resume");
        let spec = file_spec(&dir, b"AAAABBBB");
        let fetcher = FakeFetcher::new(vec![
            FakeStep::AbortAfter(b"AAAA".to_vec(), "Netz weg".into()),
            FakeStep::Deliver(b"BBBB".to_vec()),
        ]);
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        assert!(run_download(&state, ModelId::Gemma, &spec, &fetcher, true).is_err());
        assert!(matches!(state.get(ModelId::Gemma), ModelState::Error(_)));
        assert_eq!(
            fs::read(spec.partial_path()).unwrap(),
            b"AAAA",
            "Teildownload muss liegen bleiben"
        );

        run_download(&state, ModelId::Gemma, &spec, &fetcher, true).unwrap();

        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready);
        assert_eq!(fs::read(dir.path("modell.gguf")).unwrap(), b"AAAABBBB");
        assert_eq!(
            fetcher.seen_offsets(),
            vec![0, 4],
            "zweiter Aufruf resumed ab Byte 4"
        );
    }

    #[test]
    fn download_reports_progress_through_state() {
        let dir = TempDir::new("progress");
        let spec = file_spec(&dir, b"0123456789");
        let state = ModelsState::new(ModelState::Ready, ModelState::Missing);

        // Fetcher, der mitten im Download den State einfriert und prüft.
        struct Probe<'a> {
            state: &'a ModelsState,
        }
        impl ModelFetcher for Probe<'_> {
            fn fetch(
                &self,
                _url: &str,
                dest: &Path,
                progress: &mut dyn FnMut(u64, Option<u64>),
            ) -> Result<()> {
                progress(5, Some(10));
                assert_eq!(
                    self.state.get(ModelId::Gemma),
                    ModelState::Downloading { pct: 50 }
                );
                fs::write(dest, b"0123456789").unwrap();
                progress(10, Some(10));
                Ok(())
            }
        }
        // Send+Sync für den Test-Probe nicht nötig — direkt aufrufen.
        let probe = Probe { state: &state };
        run_download(&state, ModelId::Gemma, &spec, &probe, true).unwrap();
        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready);
    }

    // --- AK 2: Archiv-Pfad (Parakeet) ----------------------------------------

    #[test]
    fn archive_download_extracts_and_passes_integrity_check() {
        let files: [(&str, &[u8]); 2] = [("tokens.txt", b"a b c"), ("encoder.onnx", b"onnx")];
        let archive = build_archive(&files);
        let dir = TempDir::new("archive-ok");
        let spec = archive_spec(&dir, &archive, &files);
        let fetcher = FakeFetcher::new(vec![FakeStep::Deliver(archive.clone())]);
        let state = ModelsState::new(ModelState::Missing, ModelState::Ready);

        run_download(&state, ModelId::Parakeet, &spec, &fetcher, true).unwrap();

        assert_eq!(state.get(ModelId::Parakeet), ModelState::Ready);
        assert_eq!(fs::read(dir.path("m/tokens.txt")).unwrap(), b"a b c");
        assert!(!spec.partial_path().exists(), "Tarball muss entfernt sein");
        assert_eq!(check(&spec), ModelStatus::Ready);
    }

    #[test]
    fn archive_with_wrong_inner_hash_ends_corrupt() {
        // Archiv-Hash stimmt, aber eine entpackte Datei passt nicht zum Pin
        // (z.B. falsch gepinnter Datei-Hash) → Corrupt, nie Ready.
        let files: [(&str, &[u8]); 1] = [("tokens.txt", b"echt")];
        let archive = build_archive(&files);
        let dir = TempDir::new("archive-pin");
        let spec = ModelSpec::Archive {
            url: "https://example.invalid/m.tar.bz2".into(),
            sha256: sha_of(&archive),
            root: dir.0.clone(),
            files: vec![(dir.path("m/tokens.txt"), sha_of(b"anders"))],
        };
        let fetcher = FakeFetcher::new(vec![FakeStep::Deliver(archive)]);
        let state = ModelsState::new(ModelState::Missing, ModelState::Ready);

        let err = run_download(&state, ModelId::Parakeet, &spec, &fetcher, true).unwrap_err();

        assert!(err.to_string().contains("Integritäts-Check"));
        assert_eq!(state.get(ModelId::Parakeet), ModelState::Corrupt);
    }

    #[test]
    fn broken_archive_bytes_end_in_error_not_panic() {
        // Hash passt (auf die kaputten Bytes gepinnt), Entpacken scheitert →
        // Fehlerpfad statt Panic, Partial entfernt.
        let broken = b"kein-bzip2".to_vec();
        let dir = TempDir::new("archive-broken");
        let spec = ModelSpec::Archive {
            url: "https://example.invalid/m.tar.bz2".into(),
            sha256: sha_of(&broken),
            root: dir.0.clone(),
            files: vec![(dir.path("m/tokens.txt"), sha_of(b"x"))],
        };
        let fetcher = FakeFetcher::new(vec![FakeStep::Deliver(broken)]);
        let state = ModelsState::new(ModelState::Missing, ModelState::Ready);

        assert!(run_download(&state, ModelId::Parakeet, &spec, &fetcher, true).is_err());
        assert!(matches!(state.get(ModelId::Parakeet), ModelState::Error(_)));
        assert!(!spec.partial_path().exists());
    }

    // --- AK 3/4: Hintergrund-Task + Live-Aktivierung -------------------------

    #[test]
    fn background_gemma_download_activates_llm_modes_without_restart() {
        let dir = TempDir::new("live");
        let spec = file_spec(&dir, b"gemma");
        let state = Arc::new(ModelsState::new(
            ModelState::Ready,
            initial_state(ModelStatus::Missing, true),
        ));
        let fetcher: Arc<dyn ModelFetcher> =
            Arc::new(FakeFetcher::new(vec![FakeStep::Deliver(b"gemma".to_vec())]));

        // Vorher: nur Roh verfügbar.
        assert!(!state.llm_modes_available());
        assert!(state.mode_available(CleanupMode::Raw));
        assert!(!state.mode_available(CleanupMode::Business));

        let handle =
            spawn_background_download(Arc::clone(&state), ModelId::Gemma, spec, fetcher, true);
        handle.join().unwrap();

        // Nachher: derselbe geteilte State meldet die Modi frei — kein Neustart.
        assert!(state.llm_modes_available());
        for mode in CleanupMode::ALL {
            assert!(state.mode_available(mode), "{mode:?}");
        }
    }

    #[test]
    fn stt_gating_only_ready_parakeet_allows_ptt() {
        // Nur Roh/kein PTT solange Parakeet nicht ready (AK 5, Gating-Signal).
        for (parakeet, expected) in [
            (ModelState::Missing, false),
            (ModelState::ConsentPending, false),
            (ModelState::Downloading { pct: 99 }, false),
            (ModelState::Verifying, false),
            (ModelState::Corrupt, false),
            (ModelState::Error("x".into()), false),
            (ModelState::Ready, true),
        ] {
            let state = ModelsState::new(parakeet.clone(), ModelState::Missing);
            assert_eq!(state.stt_ready(), expected, "{parakeet:?}");
        }
    }

    #[derive(Clone, Copy)]
    enum ServerMode {
        /// Beantwortet Range-Requests korrekt mit 206.
        Ranges,
        /// Ignoriert Range, liefert immer 200 + kompletten Body.
        IgnoreRange,
        /// Immer 416 (Teildownload für den Server ungültig).
        Always416,
    }

    /// Mini-HTTP-Server für Offline-Tests des HttpFetcher (Ticket-0031):
    /// beantwortet `max_requests` Verbindungen (Connection: close) und
    /// protokolliert die gesehenen Range-Header.
    fn spawn_http_server(
        content: Vec<u8>,
        mode: ServerMode,
        max_requests: usize,
    ) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
        use std::io::BufRead as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/modell.bin", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for stream in listener.incoming().take(max_requests) {
                let Ok(mut stream) = stream else { continue };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut range: Option<String> = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(v.trim().to_string());
                    }
                }
                seen.lock().unwrap().push(range.clone());
                let respond = |stream: &mut std::net::TcpStream,
                               status: &str,
                               extra: &str,
                               body: &[u8]| {
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                };
                match (mode, range) {
                    (ServerMode::Always416, _) => {
                        respond(&mut stream, "416 Range Not Satisfiable", "", b"");
                    }
                    (ServerMode::Ranges, Some(r)) => {
                        let from: usize = r
                            .strip_prefix("bytes=")
                            .and_then(|s| s.strip_suffix('-'))
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let extra = format!(
                            "Content-Range: bytes {from}-{}/{}\r\n",
                            content.len().saturating_sub(1),
                            content.len()
                        );
                        respond(&mut stream, "206 Partial Content", &extra, &content[from..]);
                    }
                    _ => respond(&mut stream, "200 OK", "", &content),
                }
            }
        });
        (url, ranges)
    }

    #[test]
    fn http_fetcher_resumes_partial_with_range_request() {
        let content = b"0123456789ABCDEF".to_vec();
        let (url, ranges) = spawn_http_server(content.clone(), ServerMode::Ranges, 1);
        let dir = TempDir::new("http-resume");
        let dest = dir.path("d.bin");
        fs::write(&dest, b"0123").unwrap();
        let mut first_done = None;
        let mut last = (0, None);

        HttpFetcher
            .fetch(&url, &dest, &mut |done, total| {
                first_done.get_or_insert(done);
                last = (done, total);
            })
            .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), content);
        assert_eq!(ranges.lock().unwrap()[0].as_deref(), Some("bytes=4-"));
        assert!(
            first_done.unwrap() > 4,
            "Fortschritt zählt vorhandene Bytes mit"
        );
        assert_eq!(last, (16, Some(16)), "total = Offset + Rest");
    }

    #[test]
    fn http_fetcher_restarts_cleanly_when_server_ignores_range() {
        let content = b"KORREKTER INHALT".to_vec();
        let (url, _) = spawn_http_server(content.clone(), ServerMode::IgnoreRange, 1);
        let dir = TempDir::new("http-200");
        let dest = dir.path("d.bin");
        fs::write(&dest, b"MUELLDATEN").unwrap();

        HttpFetcher.fetch(&url, &dest, &mut |_, _| {}).unwrap();

        // 200 statt 206 → truncate + von vorn, kein Anhängen an den Müll.
        assert_eq!(fs::read(&dest).unwrap(), content);
    }

    #[test]
    fn http_fetcher_deletes_partial_on_416() {
        let (url, _) = spawn_http_server(Vec::new(), ServerMode::Always416, 1);
        let dir = TempDir::new("http-416");
        let dest = dir.path("d.bin");
        fs::write(&dest, b"zu viel oder ungueltig").unwrap();

        let err = HttpFetcher.fetch(&url, &dest, &mut |_, _| {}).unwrap_err();

        assert!(err.to_string().contains("416"), "{err}");
        assert!(!dest.exists(), "ungültiger Teildownload muss weg sein");
    }

    /// Echter Netz-Smoke für den Produktions-Fetcher (Redirect + Streaming +
    /// Fortschritt). Manuell: `cargo test -- --ignored`.
    #[test]
    #[ignore = "braucht Netz — manuell ausführen"]
    fn http_fetcher_streams_a_real_file_with_progress() {
        let dir = TempDir::new("http-smoke");
        let dest = dir.path("license.txt");
        let mut max_done = 0u64;
        HttpFetcher
            .fetch(
                "https://github.com/k2-fsa/sherpa-onnx/raw/master/LICENSE",
                &dest,
                // total kann fehlen (chunked) — percent() behandelt das als 0.
                &mut |done, _total| max_done = done,
            )
            .unwrap();
        let meta = fs::metadata(&dest).unwrap();
        assert!(meta.len() > 0);
        assert_eq!(
            max_done,
            meta.len(),
            "Fortschritt muss Dateigröße erreichen"
        );
    }

    #[test]
    fn needs_download_only_for_missing_corrupt_error() {
        assert!(needs_download(&ModelState::Missing));
        assert!(needs_download(&ModelState::Corrupt));
        assert!(needs_download(&ModelState::Error("x".into())));
        assert!(!needs_download(&ModelState::Ready));
        assert!(!needs_download(&ModelState::ConsentPending));
        assert!(!needs_download(&ModelState::Downloading { pct: 3 }));
        assert!(!needs_download(&ModelState::Verifying));
    }

    #[test]
    fn check_presence_skips_hashing_but_detects_partial_installs() {
        let dir = TempDir::new("presence");
        let file = dir.path("f.bin");
        fs::write(&file, b"beliebiger inhalt").unwrap();
        // Präsenz reicht — auch mit absichtlich falschem Hash Ready.
        let spec = ModelSpec::File {
            url: "https://example.invalid/f".into(),
            sha256: "definitiv-falsch".into(),
            dest: file,
        };
        assert_eq!(check_presence(&spec), ModelStatus::Ready);
        assert_eq!(check(&spec), ModelStatus::Corrupt, "voller Check urteilt");
        // Fehlend bzw. teilweise vorhanden wie beim vollen Check.
        let missing = ModelSpec::File {
            url: String::new(),
            sha256: String::new(),
            dest: dir.path("nix.bin"),
        };
        assert_eq!(check_presence(&missing), ModelStatus::Missing);
    }

    #[test]
    fn integrity_check_downgrades_presence_ready_to_corrupt() {
        let dir = TempDir::new("integrity");
        // Datei liegt am gemma-Produktionspfad, Inhalt passt nicht zum Pin.
        fs::write(dir.path(GEMMA_FILE_NAME), b"kaputt").unwrap();
        let state = Arc::new(ModelsState::from_disk_quick(&dir.0, true));
        assert_eq!(state.get(ModelId::Gemma), ModelState::Ready, "Präsenz");

        spawn_integrity_check(Arc::clone(&state), dir.0.clone())
            .join()
            .unwrap();

        assert_eq!(state.get(ModelId::Gemma), ModelState::Corrupt);
        assert_eq!(
            state.get(ModelId::Parakeet),
            ModelState::Missing,
            "fehlende Modelle bleiben unberührt"
        );
    }

    #[test]
    fn start_download_ignores_models_that_need_none() {
        // Ready/Downloading dürfen nicht neu angestoßen werden (kein State-Reset).
        let dir = TempDir::new("start-noop");
        let state = Arc::new(ModelsState::new(
            ModelState::Ready,
            ModelState::Downloading { pct: 40 },
        ));
        start_download(&state, ModelId::Parakeet, &dir.0, true);
        start_download(&state, ModelId::Gemma, &dir.0, true);
        assert_eq!(state.get(ModelId::Parakeet), ModelState::Ready);
        assert_eq!(
            state.get(ModelId::Gemma),
            ModelState::Downloading { pct: 40 }
        );
        // Ohne Consent auch bei Bedarf kein Anstoß.
        let state = Arc::new(ModelsState::new(ModelState::Missing, ModelState::Missing));
        start_needed_downloads(&state, &dir.0, false);
        assert_eq!(state.get(ModelId::Parakeet), ModelState::Missing);
        assert_eq!(state.get(ModelId::Gemma), ModelState::Missing);
    }

    #[test]
    fn any_download_running_covers_download_and_verify_phases() {
        for (gemma, expected) in [
            (ModelState::Downloading { pct: 1 }, true),
            (ModelState::Verifying, true),
            (ModelState::Missing, false),
            (ModelState::Ready, false),
            (ModelState::Corrupt, false),
            (ModelState::Error("x".into()), false),
        ] {
            let state = ModelsState::new(ModelState::Ready, gemma.clone());
            assert_eq!(state.any_download_running(), expected, "{gemma:?}");
        }
        // Beide Slots zählen — auch Parakeet allein.
        let state = ModelsState::new(ModelState::Downloading { pct: 9 }, ModelState::Missing);
        assert!(state.any_download_running());
    }

    #[test]
    fn from_disk_reflects_installed_and_missing_models() {
        let dir = TempDir::new("from-disk");
        // Nichts installiert, kein Consent → beide consent-pending.
        let state = ModelsState::from_disk(&dir.0, false);
        assert_eq!(state.get(ModelId::Parakeet), ModelState::ConsentPending);
        assert_eq!(state.get(ModelId::Gemma), ModelState::ConsentPending);
        // Mit Consent → beide missing (bereit zum Download).
        let state = ModelsState::from_disk(&dir.0, true);
        assert_eq!(state.get(ModelId::Parakeet), ModelState::Missing);
        assert_eq!(state.get(ModelId::Gemma), ModelState::Missing);
    }
}
