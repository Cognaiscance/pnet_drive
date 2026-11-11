//! The server's folder tree and file bytes.
//!
//! `tree` is the record. `blobs/` holds file bytes named by their SHA-256.
//! The same hash is stored once.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const RECORD: &str = "DRV1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub writer: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File { hash: [u8; 32], size: u64 },
}

impl Entry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    BadPath,
    MissingParent,
    Missing,
    NotAFolder,
    IsAFolder,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(err) => write!(f, "{err}"),
            StoreError::BadPath => f.write_str("path is not a relative drive path"),
            StoreError::MissingParent => f.write_str("parent folder does not exist"),
            StoreError::Missing => f.write_str("that folder is not in the drive"),
            StoreError::NotAFolder => f.write_str("parent is not a folder"),
            StoreError::IsAFolder => f.write_str("that path is a folder"),
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(err: io::Error) -> Self {
        StoreError::Io(err)
    }
}

pub struct Store {
    dir: PathBuf,
    entries: Vec<Entry>,
}

impl Store {
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        fs::create_dir_all(dir.join("blobs"))?;
        let record = dir.join("tree");
        let entries = match fs::read_to_string(&record) {
            Ok(text) => parse_tree(&text).map_err(|err| {
                io::Error::new(io::ErrorKind::InvalidData, err)
            })?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            entries,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Direct children of a folder. An empty `folder` is the root.
    pub fn list(&self, folder: &str) -> Result<Vec<Entry>, StoreError> {
        let prefix = if folder.is_empty() {
            String::new()
        } else {
            let path = check_path(folder)?;
            match self.entries.iter().find(|entry| entry.path == path) {
                Some(entry) if matches!(entry.kind, Kind::Folder) => path,
                Some(_) => return Err(StoreError::NotAFolder),
                None => return Err(StoreError::Missing),
            }
        };
        let mut children: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| is_child(&prefix, &entry.path))
            .cloned()
            .collect();
        children.sort_by(|a, b| {
            let af = matches!(a.kind, Kind::Folder);
            let bf = matches!(b.kind, Kind::Folder);
            bf.cmp(&af).then_with(|| a.name().cmp(b.name()))
        });
        Ok(children)
    }

    pub fn get(&self, path: &str) -> Result<Option<&Entry>, StoreError> {
        let path = check_path(path)?;
        Ok(self.entries.iter().find(|entry| entry.path == path))
    }

    pub fn mkdir(&mut self, path: &str, writer: &[u8; 16]) -> Result<(), StoreError> {
        let path = check_path(path)?;
        self.require_parent(&path)?;
        if let Some(existing) = self.entries.iter().find(|entry| entry.path == path) {
            return match existing.kind {
                Kind::Folder => Ok(()),
                Kind::File { .. } => Err(StoreError::NotAFolder),
            };
        }
        self.entries.push(Entry {
            path,
            kind: Kind::Folder,
            writer: *writer,
        });
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        self.write_tree()
    }

    pub fn put_file(
        &mut self,
        path: &str,
        bytes: &[u8],
        writer: &[u8; 16],
    ) -> Result<[u8; 32], StoreError> {
        let path = check_path(path)?;
        self.require_parent(&path)?;
        if let Some(existing) = self.entries.iter().find(|entry| entry.path == path) {
            if matches!(existing.kind, Kind::Folder) {
                return Err(StoreError::IsAFolder);
            }
        }
        let hash = sha256(bytes);
        self.write_blob(&hash, bytes)?;
        let size = bytes.len() as u64;
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.path == path) {
            existing.kind = Kind::File { hash, size };
            existing.writer = *writer;
        } else {
            self.entries.push(Entry {
                path,
                kind: Kind::File { hash, size },
                writer: *writer,
            });
            self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        self.write_tree()?;
        Ok(hash)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        let path = check_path(path)?;
        let Some(entry) = self.entries.iter().find(|entry| entry.path == path) else {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no such file",
            )));
        };
        let Kind::File { hash, .. } = entry.kind else {
            return Err(StoreError::IsAFolder);
        };
        let bytes = fs::read(self.blob_path(&hash))?;
        if sha256(&bytes) != hash {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "blob hash does not match the tree",
            )));
        }
        Ok(bytes)
    }

    fn require_parent(&self, path: &str) -> Result<(), StoreError> {
        let Some((parent, _)) = path.rsplit_once('/') else {
            return Ok(());
        };
        match self.entries.iter().find(|entry| entry.path == parent) {
            Some(entry) if matches!(entry.kind, Kind::Folder) => Ok(()),
            Some(_) => Err(StoreError::NotAFolder),
            None => Err(StoreError::MissingParent),
        }
    }

    fn write_blob(&self, hash: &[u8; 32], bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.blob_path(hash);
        if path.exists() {
            return Ok(());
        }
        let tmp = path.with_extension("tmp");
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    fn blob_path(&self, hash: &[u8; 32]) -> PathBuf {
        self.dir.join("blobs").join(hex(hash))
    }

    fn write_tree(&self) -> Result<(), StoreError> {
        let mut body = String::from(RECORD);
        body.push('\n');
        for entry in &self.entries {
            match entry.kind {
                Kind::Folder => {
                    body.push_str(&format!("d\t{}\t{}\n", entry.path, hex(&entry.writer)));
                }
                Kind::File { hash, size } => {
                    body.push_str(&format!(
                        "f\t{}\t{}\t{size}\t{}\n",
                        entry.path,
                        hex(&hash),
                        hex(&entry.writer)
                    ));
                }
            }
        }
        let path = self.dir.join("tree");
        let tmp = self.dir.join("tree.tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

/// A relative UTF-8 path that uses `/` and has no `.` or `..` segment.
pub fn check_path(path: &str) -> Result<String, StoreError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path.contains('\\')
        || path.contains('\n')
        || path.contains('\r')
        || path.contains('\t')
    {
        return Err(StoreError::BadPath);
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(StoreError::BadPath);
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn is_child(folder: &str, path: &str) -> bool {
    let rest = if folder.is_empty() {
        path
    } else {
        let Some(rest) = path.strip_prefix(folder) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix('/') else {
            return false;
        };
        rest
    };
    !rest.is_empty() && !rest.contains('/')
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn parse_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = text.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = from_hex(bytes[i * 2])?;
        let lo = from_hex(bytes[i * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn parse_tree(text: &str) -> Result<Vec<Entry>, String> {
    let mut lines = text.lines();
    let Some(magic) = lines.next() else {
        return Ok(Vec::new());
    };
    if magic != RECORD {
        return Err("tree record is not DRV1".into());
    }
    let mut entries = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let kind = parts.next().ok_or("short tree line")?;
        let path = parts.next().ok_or("short tree line")?;
        check_path(path).map_err(|_| "tree path is not a drive path".to_string())?;
        let entry = match kind {
            "d" => {
                let writer = parse_hex(parts.next().ok_or("short tree line")?)
                    .ok_or("bad writer")?;
                if parts.next().is_some() {
                    return Err("extra tree fields".into());
                }
                Entry {
                    path: path.to_string(),
                    kind: Kind::Folder,
                    writer,
                }
            }
            "f" => {
                let hash = parse_hex(parts.next().ok_or("short tree line")?).ok_or("bad hash")?;
                let size = parts
                    .next()
                    .ok_or("short tree line")?
                    .parse()
                    .map_err(|_| "bad size")?;
                let writer = parse_hex(parts.next().ok_or("short tree line")?)
                    .ok_or("bad writer")?;
                if parts.next().is_some() {
                    return Err("extra tree fields".into());
                }
                Entry {
                    path: path.to_string(),
                    kind: Kind::File { hash, size },
                    writer,
                }
            }
            _ => return Err("unknown tree kind".into()),
        };
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pnet-drive-store-{}-{}",
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
    fn folder_file_nested_path_and_reopen() {
        let dir = scratch();
        let writer = [9u8; 16];
        let hash = {
            let mut store = Store::open(&dir).unwrap();
            store.mkdir("notes", &writer).unwrap();
            let folder = store.get("notes").unwrap().unwrap();
            assert!(matches!(folder.kind, Kind::Folder));
            assert_eq!(folder.name(), "notes");

            let hash = store.put_file("notes/a.txt", b"hello", &writer).unwrap();
            let file = store.get("notes/a.txt").unwrap().unwrap();
            assert_eq!(file.name(), "a.txt");
            assert_eq!(
                file.kind,
                Kind::File {
                    hash,
                    size: 5
                }
            );
            assert_eq!(hash, sha256(b"hello"));

            store.mkdir("notes/2026", &writer).unwrap();
            store
                .put_file("notes/2026/plan.txt", b"later", &writer)
                .unwrap();
            assert_eq!(
                store.get("notes/2026/plan.txt").unwrap().unwrap().name(),
                "plan.txt"
            );
            hash
        };

        let store = Store::open(&dir).unwrap();
        let file = store.get("notes/a.txt").unwrap().unwrap();
        assert_eq!(file.name(), "a.txt");
        assert_eq!(file.kind, Kind::File { hash, size: 5 });
        assert_eq!(store.read_file("notes/a.txt").unwrap(), b"hello");
        assert!(matches!(
            store.get("notes/2026").unwrap().unwrap().kind,
            Kind::Folder
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dot_dot_is_rejected() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        let writer = [1u8; 16];
        assert!(matches!(
            store.mkdir("../x", &writer),
            Err(StoreError::BadPath)
        ));
        assert!(matches!(
            store.put_file("foo/../bar", b"no", &writer),
            Err(StoreError::BadPath)
        ));
        assert!(matches!(
            store.mkdir("/etc", &writer),
            Err(StoreError::BadPath)
        ));
        assert_eq!(store.len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_shows_one_folder() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        let writer = [2u8; 16];
        assert!(store.list("").unwrap().is_empty());
        store.mkdir("notes", &writer).unwrap();
        store.put_file("notes/a.txt", b"hello", &writer).unwrap();
        store.mkdir("notes/2026", &writer).unwrap();
        store
            .put_file("notes/2026/plan.txt", b"later", &writer)
            .unwrap();
        store.mkdir("photos", &writer).unwrap();

        let root: Vec<_> = store
            .list("")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name().to_string())
            .collect();
        assert_eq!(root, ["notes", "photos"]);

        let notes: Vec<_> = store
            .list("notes")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name().to_string())
            .collect();
        assert_eq!(notes, ["2026", "a.txt"]);
        assert!(matches!(store.list("missing"), Err(StoreError::Missing)));
        assert!(matches!(store.list("notes/a.txt"), Err(StoreError::NotAFolder)));
        let _ = fs::remove_dir_all(&dir);
    }
}
