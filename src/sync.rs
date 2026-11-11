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
const T_HAVE: u8 = 6;
const T_END: u8 = 7;

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
    have: HashSet<String>,
}

impl Inbox {
    pub fn new() -> Self {
        Self {
            partial: HashMap::new(),
            have: HashSet::new(),
        }
    }

    /// Applies one report. A finished index returns the files and folders the client does not have.
    pub fn apply(
        &mut self,
        store: &mut Store,
        packet: &[u8],
        now_ms: u64,
    ) -> Result<Vec<Change>, StoreError> {
        if packet.len() < 10 || packet[0] != VER {
            return Err(StoreError::BadPath);
        }
        let body = &packet[10..];
        match packet[1] {
            T_MKDIR => {
                let path = take_path(body)?.0;
                store.mkdir(&path, &[0u8; 16])?;
                Ok(Vec::new())
            }
            T_DELETE => {
                let path = take_path(body)?.0;
                match store.trash_file(&path, now_ms) {
                    Ok(()) => Ok(Vec::new()),
                    Err(StoreError::IsAFolder) => store.remove_folder(&path).map(|()| Vec::new()),
                    Err(err) => Err(err),
                }
            }
            T_HAVE => {
                let path = take_path(body)?.0;
                self.have.insert(path);
                Ok(Vec::new())
            }
            T_END => {
                let have = std::mem::take(&mut self.have);
                missing(store, &have)
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
                store.put_file(&path, &bytes, &[0u8; 16])?;
                Ok(Vec::new())
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
                Ok(Vec::new())
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
                    store.put_file(&path, &bytes, &[0u8; 16])?;
                    Ok(Vec::new())
                } else {
                    Ok(Vec::new())
                }
            }
            _ => Err(StoreError::BadPath),
        }
    }
}

/// Brings a client in line with the live tree.
///
/// Paths the store has and the client does not are created. A path the client
/// still has that is in trash is deleted. A local file the server has never stored is left alone.
pub fn missing(store: &Store, have: &HashSet<String>) -> Result<Vec<Change>, StoreError> {
    let live: HashSet<&str> = store.entries().iter().map(|entry| entry.path.as_str()).collect();
    let trashed: HashSet<&str> = store.trash().iter().map(|item| item.path.as_str()).collect();
    let mut changes = Vec::new();
    for entry in store.entries() {
        if have.contains(&entry.path) {
            continue;
        }
        match entry.kind {
            store::Kind::Folder => changes.push(Change::Mkdir(entry.path.clone())),
            store::Kind::File { .. } => {
                let bytes = store.read_file(&entry.path)?;
                changes.push(Change::File {
                    path: entry.path.clone(),
                    bytes,
                });
            }
        }
    }
    let mut removed: Vec<String> = have
        .iter()
        .filter(|path| trashed.contains(path.as_str()) && !live.contains(path.as_str()))
        .cloned()
        .collect();
    removed.sort_by(|a, b| b.len().cmp(&a.len()).then(b.cmp(a)));
    for path in removed {
        changes.push(Change::Delete(path));
    }
    changes.sort_by(|a, b| {
        let key = |change: &Change| match change {
            Change::Mkdir(path) => (0, path.len(), path.clone()),
            Change::File { path, .. } => (1, path.len(), path.clone()),
            Change::Delete(path) => (2, usize::MAX - path.len(), path.clone()),
        };
        key(a).cmp(&key(b))
    });
    Ok(changes)
}

pub fn index_packets(paths: &[String]) -> Vec<Vec<u8>> {
    let mut packets: Vec<_> = paths
        .iter()
        .map(|path| packet(T_HAVE, &path_body(path)))
        .collect();
    packets.push(packet(T_END, &[]));
    packets
}

pub fn write_local(root: &Path, change: &Change) -> io::Result<()> {
    match change {
        Change::Mkdir(path) => fs::create_dir_all(root.join(path)),
        Change::File { path, bytes } => {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(full, bytes)
        }
        Change::Delete(path) => {
            let full = root.join(path);
            if full.is_dir() {
                fs::remove_dir_all(full)
            } else {
                match fs::remove_file(full) {
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            }
        }
    }
}

/// Writes server chunks into the local folder.
pub struct Downloader {
    partial: HashMap<String, Partial>,
}

impl Downloader {
    pub fn new() -> Self {
        Self {
            partial: HashMap::new(),
        }
    }

    /// Paths written by this packet. The caller marks them quiet.
    pub fn apply(&mut self, root: &Path, packet: &[u8]) -> io::Result<Vec<String>> {
        if packet.len() < 10 || packet[0] != VER {
            return Ok(Vec::new());
        }
        let body = &packet[10..];
        match packet[1] {
            T_MKDIR => {
                let path = take_path(body).map_err(store_io)?.0;
                write_local(root, &Change::Mkdir(path.clone()))?;
                Ok(vec![path])
            }
            T_DELETE => {
                let path = take_path(body).map_err(store_io)?.0;
                write_local(root, &Change::Delete(path.clone()))?;
                Ok(vec![path])
            }
            T_START => {
                let (path, rest) = take_path(body).map_err(store_io)?;
                if rest.len() != 8 + 32 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "short start"));
                }
                let size = u64::from_be_bytes(rest[..8].try_into().unwrap());
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&rest[8..40]);
                self.partial.insert(path, Partial { size, hash, buf: Vec::new() });
                Ok(Vec::new())
            }
            T_CHUNK => {
                let (path, rest) = take_path(body).map_err(store_io)?;
                if rest.len() < 8 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "short chunk"));
                }
                let offset = u64::from_be_bytes(rest[..8].try_into().unwrap());
                let part = self
                    .partial
                    .get_mut(&path)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "chunk without start"))?;
                if offset != part.buf.len() as u64 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "chunk out of order"));
                }
                part.buf.extend_from_slice(&rest[8..]);
                if part.buf.len() as u64 == part.size && sha256(&part.buf) == part.hash {
                    let bytes = part.buf.clone();
                    self.partial.remove(&path);
                    write_local(root, &Change::File { path: path.clone(), bytes })?;
                    Ok(vec![path])
                } else {
                    Ok(Vec::new())
                }
            }
            _ => Ok(Vec::new()),
        }
    }
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

fn store_io(err: StoreError) -> io::Error {
    io::Error::other(err.to_string())
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

    #[test]
    fn downloads_a_server_file_without_reporting_it() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        store.mkdir("notes", &[0u8; 16]).unwrap();
        store.mkdir("notes/2026", &[0u8; 16]).unwrap();
        store.put_file("notes/a.txt", b"hello", &[0u8; 16]).unwrap();
        store.put_file("notes/2026/plan.txt", b"later", &[0u8; 16]).unwrap();

        let root = scratch();
        local::ensure(&root).unwrap();
        fs::write(root.join("keep.txt"), b"mine").unwrap();
        let have: HashSet<String> = ["keep.txt".to_string()].into_iter().collect();
        let changes = missing(&store, &have).unwrap();
        assert_eq!(changes.len(), 4);

        let mut down = Downloader::new();
        let mut watch = Watch::new();
        watch.poll(&root).unwrap();
        watch.commit();
        for change in &changes {
            for packet in encode(change, false) {
                let written = down.apply(&root, &packet).unwrap();
                for path in written {
                    watch.quiet(&path);
                }
            }
        }
        assert_eq!(fs::read(root.join("notes/a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(root.join("notes/2026/plan.txt")).unwrap(), b"later");
        assert!(root.join("notes/2026").is_dir());
        let reported = watch.poll(&root).unwrap();
        assert!(reported.is_empty());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_clients_follow_add_delete_and_restore() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        let root_a = scratch();
        let root_b = scratch();
        local::ensure(&root_a).unwrap();
        local::ensure(&root_b).unwrap();
        let mut watch_a = Watch::new();
        let mut watch_b = Watch::new();
        watch_a.poll(&root_a).unwrap();
        watch_a.commit();
        watch_b.poll(&root_b).unwrap();
        watch_b.commit();
        let mut down_b = Downloader::new();

        fs::create_dir(root_a.join("notes")).unwrap();
        fs::write(root_a.join("notes/a.txt"), b"hello").unwrap();
        let added = watch_a.poll(&root_a).unwrap();
        watch_a.commit();
        deliver(&mut store, &added);
        pull(&store, &root_b, &mut watch_b, &mut down_b);
        assert_eq!(fs::read(root_b.join("notes/a.txt")).unwrap(), b"hello");
        assert!(watch_b.poll(&root_b).unwrap().is_empty());
        watch_b.commit();

        fs::remove_file(root_a.join("notes/a.txt")).unwrap();
        let deleted = watch_a.poll(&root_a).unwrap();
        watch_a.commit();
        deliver(&mut store, &deleted);
        assert_eq!(store.trash()[0].path, "notes/a.txt");
        pull(&store, &root_b, &mut watch_b, &mut down_b);
        assert!(!root_b.join("notes/a.txt").exists());
        assert!(watch_b.poll(&root_b).unwrap().is_empty());
        watch_b.commit();

        store.restore("notes/a.txt").unwrap();
        let mut down_a = Downloader::new();
        pull(&store, &root_a, &mut watch_a, &mut down_a);
        pull(&store, &root_b, &mut watch_b, &mut down_b);
        assert_eq!(fs::read(root_a.join("notes/a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(root_b.join("notes/a.txt")).unwrap(), b"hello");
        assert!(watch_a.poll(&root_a).unwrap().is_empty());
        assert!(watch_b.poll(&root_b).unwrap().is_empty());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&root_a);
        let _ = fs::remove_dir_all(&root_b);
    }

    fn pull(store: &Store, root: &Path, watch: &mut Watch, down: &mut Downloader) {
        let have = local::list(root)
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        for change in missing(store, &have).unwrap() {
            for packet in encode(&change, false) {
                for path in down.apply(root, &packet).unwrap() {
                    watch.quiet(&path);
                }
            }
        }
    }
}
