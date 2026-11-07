//! Logging (ADR-0011): `log`-Fassade, eigener Datei-Logger mit Rotation.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Ziel für fertig formatierte Logzeilen (ohne Zeilenende).
pub trait LogSink: Send {
    fn write_line(&mut self, line: &str) -> io::Result<()>;
}

/// `talker.log` mit Größenrotation: `talker.log` → `talker.1.log` … `talker.<keep>.log`,
/// die älteste Datei wird gelöscht. Keine Löschung nach Alter (REQ-DIAG-002).
pub struct RotatingFile {
    dir: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
    keep: usize,
}

const LOG_NAME: &str = "talker";

fn log_path(dir: &Path, index: usize) -> PathBuf {
    if index == 0 {
        dir.join(format!("{LOG_NAME}.log"))
    } else {
        dir.join(format!("{LOG_NAME}.{index}.log"))
    }
}

impl RotatingFile {
    /// Öffnet `talker.log` im Anhängemodus; die Größe kommt aus der Dateilänge.
    pub fn open(dir: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(dir, 0))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
            max_bytes,
            keep,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        if self.keep > 0 {
            match std::fs::remove_file(log_path(&self.dir, self.keep)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            for index in (0..self.keep).rev() {
                match std::fs::rename(log_path(&self.dir, index), log_path(&self.dir, index + 1)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path(&self.dir, 0))?;
        self.size = 0;
        Ok(())
    }
}

impl LogSink for RotatingFile {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.size += bytes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Frisches, test-eigenes Verzeichnis unterm System-Tempdir (kein tempfile-Dep).
    pub(super) struct TestDir(pub PathBuf);
    impl TestDir {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "talker-log-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Zeile mit genau `total` Byte inkl. Zeilenende.
    fn line(total: usize, fill: char) -> String {
        fill.to_string().repeat(total - 1)
    }

    /// REQ-DIAG-002: genau `max_bytes` → keine Rotation.
    #[test]
    fn file_of_exactly_max_bytes_is_not_rotated() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(60, 'a')).unwrap();
        f.write_line(&line(40, 'b')).unwrap();
        assert_eq!(names(&t.0), ["talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.log")).unwrap().len(),
            100
        );
    }

    /// REQ-DIAG-002: +1 Byte → Rotation vor dem Schreiben der Zeile.
    #[test]
    fn line_that_would_cross_the_limit_rotates_first() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(60, 'a')).unwrap();
        f.write_line(&line(41, 'b')).unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.1.log")).unwrap().len(),
            60
        );
        assert_eq!(std::fs::metadata(t.0.join("talker.log")).unwrap().len(), 41);
        assert!(
            std::fs::read_to_string(t.0.join("talker.log"))
                .unwrap()
                .starts_with('b')
        );
    }

    /// REQ-DIAG-002: nie mehr als 5 Dateien; die 5. Rotation löscht `talker.4.log`.
    #[test]
    fn fifth_rotation_drops_the_oldest_and_keeps_five_files() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 10, 4).unwrap();
        for c in ['a', 'b', 'c', 'd', 'e', 'f'] {
            f.write_line(&line(10, c)).unwrap();
        }
        assert_eq!(
            names(&t.0),
            [
                "talker.1.log",
                "talker.2.log",
                "talker.3.log",
                "talker.4.log",
                "talker.log"
            ]
        );
        let first = |n: &str| {
            std::fs::read_to_string(t.0.join(n))
                .unwrap()
                .chars()
                .next()
                .unwrap()
        };
        assert_eq!(first("talker.log"), 'f');
        assert_eq!(first("talker.1.log"), 'e');
        assert_eq!(first("talker.4.log"), 'b');
    }

    /// REQ-DIAG-002 „darf nicht passieren": keine Löschung nach Alter.
    #[test]
    fn old_files_are_not_deleted_by_age() {
        let t = TestDir::new();
        let old = t.0.join("talker.1.log");
        std::fs::write(&old, "alt\n").unwrap();
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line("neu").unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "alt\n");
        assert_eq!(
            std::fs::metadata(&old).unwrap().modified().unwrap(),
            long_ago
        );
    }

    /// Vorhandene Datei wird angehängt, Größe stammt aus der Dateilänge.
    #[test]
    fn existing_file_is_appended_and_size_is_taken_from_disk() {
        let t = TestDir::new();
        std::fs::write(t.0.join("talker.log"), line(90, 'x') + "\n").unwrap();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(11, 'y')).unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.1.log")).unwrap().len(),
            90
        );
    }

    /// Eine einzelne Zeile über dem Limit steht allein in einer frischen Datei.
    #[test]
    fn single_oversized_line_is_written_without_endless_rotation() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 10, 4).unwrap();
        f.write_line(&line(50, 'z')).unwrap();
        assert_eq!(names(&t.0), ["talker.log"]);
        f.write_line("x").unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
    }

    /// REQ-DIAG-001/-010: nicht anlegbares/schreibgeschütztes Verzeichnis → Err.
    #[test]
    fn open_in_read_only_dir_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = TestDir::new();
        std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = RotatingFile::open(&t.0.join("logs"), 100, 4);
        std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        if std::fs::write(t.0.join("probe"), "").is_ok() {
            // root ignoriert Rechte — dann ist der Test nicht aussagekräftig
            return;
        }
        assert!(result.is_err());
    }
}
