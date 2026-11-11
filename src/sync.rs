//! Local changes the client reports to the holder.
//!
//! The client polls its folder. A path it just wrote itself is quiet for one
//! poll and is not reported. File bytes move in chunks of at most 2048.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::store::{self, Store, StoreError};

pub const VER: u8 = 1;
pub const CHUNK: usize = 2048;
const T_MKDIR: u8 = 1;
const T_DELETE: u8 = 2;
const T_LINK: u8 = 3;
const T_START: u8 = 4;
const T_CHUNK: u8 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Meta {
    folder: bool,
    size: u64,
    hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Mkdir(String),
    File { path: String, bytes: Vec<u8> },
    Delete(String),
}

pub struct Watch {
    prev: BTreeMap<String, Meta>,
    quiet: HashSet<String>,
    pending: Option<BTreeMap<String, Meta>>,
}

impl Watch {
    pub fn new() -> Self {
        Self {
            prev: BTreeMap::new(),
            quiet: HashSet::new(),
            pending: None,
        }
    }

    /// Keep the snapshot from the last poll. Call this after the report is sent.
    pub fn commit(&mut self) {
        if let Some(next) = self.pending.take() {
            self.quiet.retain(|path| !next.contains_key(path));
            self.prev = next;
        }
    }

    /// The next poll will not report this path. Used for a file this client just wrote.
    pub fn quiet(&mut self, path: &str) {
        self.quiet.insert(path.to_string());
    }

    pub fn poll(&mut self, root: &Path) -> io::Result<Vec<Change>> {
        let next = snapshot(root)?;
        let mut changes = Vec::new();
        for (path, meta) in &next {
            if self.quiet.contains(path) {
                continue;
            }
            match self.prev.get(path) {
                None if meta.folder => changes.push(Change::Mkdir(path.clone())),
                None => {
                    let bytes = fs::read(root.join(path))?;
                    changes.push(Change::File {
                        path: path.clone(),
                        bytes,
                    });
                }
                Some(old) if !meta.folder && old.hash != meta.hash => {
                    let bytes = fs::read(root.join(path))?;
                    changes.push(Change::File {
                        path: path.clone(),
                        bytes,
                    });
                }
                _ => {}
            }
        }
        let mut deleted: Vec<String> = self
            .prev
            .keys()
            .filter(|path| !next.contains_key(*path) && !self.quiet.contains(*path))
            .cloned()
            .collect();
        deleted.sort_by(|a, b| b.len().cmp(&a.len()).then(b.cmp(a)));
        for path in deleted {
            changes.push(Change::Delete(path));
        }
        self.pending = Some(next);
        changes.sort_by_key(|change| match change {
            Change::Mkdir(path) => (0, path.clone()),
            Change::File { path, .. } => (1, path.clone()),
            Change::Delete(path) => (2, path.clone()),
        });
        Ok(changes)
    }
}

impl Default for Watch {
    fn default() -> Self {
        Self::new()
    }
}

pub fn encode(change: &Change, have_hash: bool) -> Vec<Vec<u8>> {
    match change {
        Change::Mkdir(path) => vec![packet(T_MKDIR, &path_body(path))],
        Change::Delete(path) => vec![packet(T_DELETE, &path_body(path))],
        Change::File { path, bytes } => {
            let hash = sha256(bytes);
            if have_hash {
                let mut body = path_body(path);
                body.extend_from_slice(&hash);
                body.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
                vec![packet(T_LINK, &body)]
            } else {
                let mut packets = Vec::new();
                let mut body = path_body(path);
                body.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
                body.extend_from_slice(&hash);
                packets.push(packet(T_START, &body));
                for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
                    let mut body = path_body(path);
                    body.extend_from_slice(&((index * CHUNK) as u64).to_be_bytes());
                    body.extend_from_slice(chunk);
                    packets.push(packet(T_CHUNK, &body));
                }
                if bytes.is_empty() {
                    let mut body = path_body(path);
                    body.extend_from_slice(&0u64.to_be_bytes());
                    packets.push(packet(T_CHUNK, &body));
                }
                packets
            }
        }
    }
}

struct Partial {
    size: u64,
    hash: [u8; 32],
    buf: Vec<u8>,
}

pub struct Inbox {
    partial: HashMap<String, Partial>,
}

impl Inbox {
    pub fn new() -> Self {
        Self {
            partial: HashMap::new(),
        }
    }

    pub fn apply(&mut self, store: &mut Store, packet: &[u8], now_ms: u64) -> Result<(), StoreError> {
        if packet.len() < 10 || packet[0] != VER {
            return Err(StoreError::BadPath);
        }
        let body = &packet[10..];
        match packet[1] {
            T_MKDIR => {
                let path = take_path(body)?.0;
                store.mkdir(&path, &[0u8; 16])
            }
            T_DELETE => {
                let path = take_path(body)?.0;
                match store.trash_file(&path, now_ms) {
                    Ok(()) => Ok(()),
                    Err(StoreError::IsAFolder) => store.remove_folder(&path),
                    Err(err) => Err(err),
                }
            }
            T_LINK => {
                let (path, rest) = take_path(body)?;
                if rest.len() != 32 + 8 {
                    return Err(StoreError::BadPath);
                }
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&rest[..32]);
                let Some(bytes) = store.read_blob(&hash) else {
                    return Err(StoreError::Missing);
                };
                store.put_file(&path, &bytes, &[0u8; 16]).map(|_| ())
            }
            T_START => {
                let (path, rest) = take_path(body)?;
                if rest.len() != 8 + 32 {
                    return Err(StoreError::BadPath);
                }
                let size = u64::from_be_bytes(rest[..8].try_into().unwrap());
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&rest[8..40]);
                self.partial.insert(
                    path,
                    Partial {
                        size,
                        hash,
                        buf: Vec::new(),
                    },
                );
                Ok(())
            }
            T_CHUNK => {
                let (path, rest) = take_path(body)?;
                if rest.len() < 8 {
                    return Err(StoreError::BadPath);
                }
                let offset = u64::from_be_bytes(rest[..8].try_into().unwrap());
                let data = &rest[8..];
                let part = self.partial.get_mut(&path).ok_or(StoreError::Missing)?;
                if offset != part.buf.len() as u64 {
                    return Err(StoreError::BadPath);
                }
                part.buf.extend_from_slice(data);
                if part.buf.len() as u64 == part.size && sha256(&part.buf) == part.hash {
                    let bytes = part.buf.clone();
                    self.partial.remove(&path);
                    store.put_file(&path, &bytes, &[0u8; 16]).map(|_| ())
                } else {
                    Ok(())
                }
            }
            _ => Err(StoreError::BadPath),
        }
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

fn snapshot(root: &Path) -> io::Result<BTreeMap<String, Meta>> {
    let mut map = BTreeMap::new();
    for entry in crate::local::list(root)? {
        if entry.folder {
            map.insert(
                entry.path,
                Meta {
                    folder: true,
                    size: 0,
                    hash: [0u8; 32],
                },
            );
        } else {
            let bytes = fs::read(root.join(&entry.path))?;
            map.insert(
                entry.path,
                Meta {
                    folder: false,
                    size: bytes.len() as u64,
                    hash: sha256(&bytes),
                },
            );
        }
    }
    Ok(map)
}

fn packet(typ: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + body.len());
    out.push(VER);
    out.push(typ);
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn path_body(path: &str) -> Vec<u8> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(2 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

fn take_path(body: &[u8]) -> Result<(String, &[u8]), StoreError> {
    if body.len() < 2 {
        return Err(StoreError::BadPath);
    }
    let len = u16::from_be_bytes([body[0], body[1]]) as usize;
    if body.len() < 2 + len {
        return Err(StoreError::BadPath);
    }
    let path = std::str::from_utf8(&body[2..2 + len]).map_err(|_| StoreError::BadPath)?;
    let path = store::check_path(path)?;
    Ok((path, &body[2 + len..]))
}

pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    sha256(bytes)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local;

    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pnet-drive-sync-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn deliver(store: &mut Store, changes: &[Change]) {
        let mut inbox = Inbox::new();
        for change in changes {
            let have = match change {
                Change::File { bytes, .. } => store.read_blob(&sha256(bytes)).is_some(),
                _ => false,
            };
            for packet in encode(change, have) {
                inbox.apply(store, &packet, 50).unwrap();
            }
        }
    }

    #[test]
    fn reports_new_changed_folder_and_delete_but_not_a_quiet_write() {
        let root = scratch();
        local::ensure(&root).unwrap();
        let mut watch = Watch::new();
        assert!(watch.poll(&root).unwrap().is_empty());
        watch.commit();

        fs::create_dir(root.join("notes")).unwrap();
        fs::write(root.join("notes/a.txt"), b"hello").unwrap();
        let first = watch.poll(&root).unwrap();
        watch.commit();
        assert!(matches!(&first[0], Change::Mkdir(path) if path == "notes"));
        assert!(matches!(&first[1], Change::File { path, bytes } if path == "notes/a.txt" && bytes == b"hello"));

        let store_dir = scratch();
        let mut store = Store::open(&store_dir).unwrap();
        deliver(&mut store, &first);
        assert!(store.get("notes").unwrap().is_some());
        assert_eq!(store.read_file("notes/a.txt").unwrap(), b"hello");

        fs::write(root.join("notes/a.txt"), b"hello!").unwrap();
        let changed = watch.poll(&root).unwrap();
        watch.commit();
        assert!(changed.iter().any(|change| matches!(change, Change::File { path, bytes } if path == "notes/a.txt" && bytes == b"hello!")));
        deliver(&mut store, &changed);
        assert_eq!(store.read_file("notes/a.txt").unwrap(), b"hello!");

        fs::create_dir(root.join("notes/2026")).unwrap();
        fs::remove_file(root.join("notes/a.txt")).unwrap();
        fs::write(root.join("notes/b.txt"), b"new").unwrap();
        watch.quiet("notes/b.txt");
        let second = watch.poll(&root).unwrap();
        watch.commit();
        assert!(second.iter().any(|change| matches!(change, Change::Mkdir(path) if path == "notes/2026")));
        assert!(second.iter().any(|change| matches!(change, Change::Delete(path) if path == "notes/a.txt")));
        assert!(!second.iter().any(|change| matches!(change, Change::File { path, .. } if path == "notes/b.txt")));
        deliver(&mut store, &second);
        assert!(store.get("notes/a.txt").unwrap().is_none());
        assert!(matches!(
            store.get("notes/2026").unwrap().unwrap().kind,
            crate::store::Kind::Folder
        ));
        assert_eq!(store.trash()[0].path, "notes/a.txt");

        let third = watch.poll(&root).unwrap();
        watch.commit();
        assert!(!third.iter().any(|change| matches!(change, Change::File { path, .. } if path == "notes/b.txt")));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&store_dir);
    }

    #[test]
    fn same_hash_is_not_sent_as_chunks() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        store.mkdir("notes", &[0u8; 16]).unwrap();
        store.put_file("notes/a.txt", b"hello", &[0u8; 16]).unwrap();
        let change = Change::File {
            path: "notes/copy.txt".into(),
            bytes: b"hello".to_vec(),
        };
        let packets = encode(&change, store.read_blob(&sha256(b"hello")).is_some());
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][1], T_LINK);
        Inbox::new().apply(&mut store, &packets[0], 1).unwrap();
        assert_eq!(store.read_file("notes/copy.txt").unwrap(), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }
}
