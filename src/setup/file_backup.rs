//! File snapshots used by the installer before it makes any persistent changes.
//! Captures stay in memory; passwords are never written to a temporary backup.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub struct FileBackup {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl FileBackup {
    pub fn capture(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut files = Vec::new();
        for path in paths {
            if files.iter().any(|(p, _)| p == &path) {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    return Err(e).with_context(|| format!("Failed to back up {}", path.display()))
                }
            };
            files.push((path, bytes));
        }
        Ok(Self { files })
    }

    /// Restore all paths, continue after errors so one locked file does not
    /// prevent recovery of the remaining config and executable files.
    pub fn restore(&self) -> Result<()> {
        let mut failures = Vec::new();
        for (path, before) in self.files.iter().rev() {
            if let Err(e) = restore_file(path, before.as_deref()) {
                failures.push(format!("{}: {e:#}", path.display()));
            }
        }
        anyhow::ensure!(
            failures.is_empty(),
            "File rollback failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}

fn restore_file(path: &Path, before: Option<&[u8]>) -> Result<()> {
    match before {
        Some(bytes) => {
            // In particular, do not rewrite the currently running setup when
            // it was captured but never changed.
            if std::fs::read(path).is_ok_and(|current| current == bytes) {
                return Ok(());
            }
            replace_file(path, bytes).context("Failed to restore original bytes")?;
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("Failed to remove newly installed file"),
        },
    }
    Ok(())
}

/// Finish writing and syncing a sibling file before touching the destination.
/// Windows rename cannot replace an existing executable, so retain the previous
/// file under a sibling name until the replacement has been moved into place.
pub fn replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    replace_file_with(path, bytes, |from, to| std::fs::rename(from, to))
}

fn replace_file_with(
    path: &Path,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file(),
            "Destination is not a regular file: {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to inspect destination"),
    }
    let parent = path.parent().context("File destination has no parent")?;
    let (staged, mut file) = loop {
        let name = format!(
            ".gdut-install-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let staged = parent.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
        {
            Ok(file) => break (staged, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("Failed to create staged file"),
        }
    };
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let previous = staged.with_extension("old");
        anyhow::ensure!(!previous.exists(), "Staging backup already exists");
        let had_previous = match std::fs::rename(path, &previous) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e).context("Failed to retain previous file"),
        };
        if let Err(error) = publish(&staged, path) {
            if had_previous {
                std::fs::rename(&previous, path).with_context(|| {
                    format!(
                        "Replacement failed ({error}); original retained at {}",
                        previous.display()
                    )
                })?;
            }
            return Err(error).context("Failed to publish staged file");
        }
        if had_previous {
            std::fs::remove_file(previous).context("Failed to remove previous staged file")?;
        }
        Ok(())
    })();
    // All handles have been closed before cleanup (required on Windows).
    let _ = std::fs::remove_file(staged);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publish_restores_existing_destination_and_cleans_staging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gdut-net.exe");
        std::fs::write(&path, b"old complete exe").unwrap();
        assert!(replace_file_with(&path, b"new exe", |_, _| {
            Err(std::io::Error::other("injected rename failure"))
        })
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old complete exe");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn directory_destination_is_never_moved_or_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep.txt"), b"keep").unwrap();
        assert!(replace_file(&path, b"new exe").is_err());
        assert!(path.is_dir());
        assert_eq!(std::fs::read(path.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
