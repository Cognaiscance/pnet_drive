//! The folder on this device. Listing uses the same path rules as the server.
//!
//! This module does not send those paths anywhere.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::store;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEntry {
    pub path: String,
    pub folder: bool,
    pub size: u64,
}

pub fn folder_dir() -> PathBuf {
    if let Ok(path) = std::env::var("DRIVE_FOLDER") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join("Drive");
        }
    }
    PathBuf::from("Drive")
}

pub fn ensure(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// Files and folders under `dir`, relative to `dir`. A symlink is skipped.
pub fn list(dir: &Path) -> io::Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    walk(dir, dir, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<LocalEntry>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\')
        {
            continue;
        }
        let meta = match fs::symlink_metadata(item.path()) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let rel = item.path();
        let rel = rel.strip_prefix(root).unwrap_or(&rel);
        let Some(path) = rel.to_str() else {
            continue;
        };
        let path = path.replace('\\', "/");
        if store::check_path(&path).is_err() {
            continue;
        }
        if meta.is_dir() {
            out.push(LocalEntry {
                path: path.clone(),
                folder: true,
                size: 0,
            });
            walk(root, &item.path(), out)?;
        } else if meta.is_file() {
            out.push(LocalEntry {
                path,
                folder: false,
                size: meta.len(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pnet-drive-local-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn creates_and_lists_nested_files() {
        let dir = scratch();
        assert!(!dir.exists());
        ensure(&dir).unwrap();
        assert!(dir.is_dir());
        fs::create_dir_all(dir.join("notes/2026")).unwrap();
        fs::write(dir.join("notes/a.txt"), b"hello").unwrap();
        fs::write(dir.join("notes/2026/plan.txt"), b"later").unwrap();

        let listed = list(&dir).unwrap();
        let paths: Vec<_> = listed.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(
            paths,
            ["notes", "notes/2026", "notes/2026/plan.txt", "notes/a.txt"]
        );
        let file = listed.iter().find(|entry| entry.path == "notes/a.txt").unwrap();
        assert!(!file.folder);
        assert_eq!(file.size, 5);
        assert!(paths.iter().all(|path| store::check_path(path).is_ok()));
        assert!(paths.iter().all(|path| !path.contains("..")));
        let _ = fs::remove_dir_all(&dir);
    }
}
