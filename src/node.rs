//! Register this process with the local pNet node.
//!
//! Register, get-data, and send against the local pNet node.
//!
//! Inbound app payloads arrive on the push port.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::{SocketAddr, UdpSocket};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::APP_PROTOCOL;

const OP_REGISTER: u8 = 0x00;
const OP_GET_DATA: u8 = 0x02;
const OP_SEND: u8 = 0x03;
const OP_PUSH: u8 = 0x04;
const STATUS_OK: u8 = 0x00;

#[derive(Debug)]
pub enum NodeError {
    Timeout,
    Io(io::Error),
    Rejected { code: Option<u8> },
    Unexpected,
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Timeout => f.write_str("no reply from the local node"),
            NodeError::Io(err) => write!(f, "{err}"),
            NodeError::Rejected { code: Some(code) } => {
                write!(f, "node rejected the request ({code:#04x})")
            }
            NodeError::Rejected { code: None } => f.write_str("node rejected the request"),
            NodeError::Unexpected => f.write_str("unexpected reply from the local node"),
        }
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(path) = std::env::var("DRIVE_DIR") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".pnet/drive");
        }
    }
    PathBuf::from(".pnet/drive")
}

pub fn token_path(dir_name: &str) -> PathBuf {
    if let Ok(path) = std::env::var("DRIVE_TOKEN_FILE") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".pnet")
                .join(dir_name)
                .join("token");
        }
    }
    PathBuf::from(format!("{dir_name}-token"))
}

pub fn save_token(path: &Path, token: &[u8; 16]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(token)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn load_token(path: &Path) -> io::Result<Option<[u8; 16]>> {
    match fs::read(path) {
        Ok(bytes) if bytes.len() == 16 => {
            let mut token = [0u8; 16];
            token.copy_from_slice(&bytes);
            Ok(Some(token))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token file must be 16 bytes",
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub struct Push {
    pub sender_app: [u8; 16],
    pub payload: Vec<u8>,
}

pub fn parse_push(buf: &[u8]) -> Option<Push> {
    if buf.len() < 17 || buf[0] != OP_PUSH {
        return None;
    }
    let mut sender_app = [0u8; 16];
    sender_app.copy_from_slice(&buf[1..17]);
    Some(Push {
        sender_app,
        payload: buf[17..].to_vec(),
    })
}

pub fn bind_push(port: u16) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

fn push_str(buf: &mut Vec<u8>, text: &str) {
    let len = text.len().min(255);
    buf.push(len as u8);
    buf.extend_from_slice(&text.as_bytes()[..len]);
}

pub struct NodeClient {
    socket: UdpSocket,
}

impl NodeClient {
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.connect(addr)?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        Ok(Self { socket })
    }

    pub fn register(&self, alias: &str, push_port: u16) -> Result<[u8; 16], NodeError> {
        let mut buf = vec![OP_REGISTER];
        push_str(&mut buf, alias);
        buf.extend_from_slice(&push_port.to_be_bytes());
        push_str(&mut buf, APP_PROTOCOL);
        let reply = self.round_trip(&buf)?;
        if reply.len() != 17 || reply[0] != STATUS_OK {
            return Err(NodeError::Unexpected);
        }
        let mut token = [0u8; 16];
        token.copy_from_slice(&reply[1..17]);
        Ok(token)
    }

    pub fn get_data(&self, token: &[u8; 16]) -> Result<Vec<u8>, NodeError> {
        let mut buf = vec![OP_GET_DATA];
        buf.extend_from_slice(token);
        self.round_trip(&buf)
    }

    pub fn send(
        &self,
        token: &[u8; 16],
        dest_device: &[u8; 16],
        dest_app: &[u8; 16],
        payload: &[u8],
    ) -> Result<(), NodeError> {
        let mut buf = Vec::with_capacity(1 + 48 + payload.len());
        buf.push(OP_SEND);
        buf.extend_from_slice(token);
        buf.extend_from_slice(dest_device);
        buf.extend_from_slice(dest_app);
        buf.extend_from_slice(payload);
        self.socket.send(&buf).map_err(NodeError::Io)?;
        let mut reply = [0u8; 64];
        match self.socket.recv(&mut reply) {
            Ok(n) if n > 0 && reply[0] == 0x00 => Ok(()),
            Ok(n) if n > 0 && reply[0] == 0x01 => Err(NodeError::Rejected {
                code: reply.get(1).copied(),
            }),
            Ok(_) => Err(NodeError::Unexpected),
            Err(err)
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(())
            }
            Err(err) => Err(NodeError::Io(err)),
        }
    }

    fn round_trip(&self, packet: &[u8]) -> Result<Vec<u8>, NodeError> {
        self.socket.send(packet).map_err(NodeError::Io)?;
        let mut reply = vec![0u8; 65536];
        match self.socket.recv(&mut reply) {
            Ok(0) => Err(NodeError::Unexpected),
            Ok(n) => Ok(reply[..n].to_vec()),
            Err(err)
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut =>
            {
                Err(NodeError::Timeout)
            }
            Err(err) => Err(NodeError::Io(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip() {
        let dir = std::env::temp_dir().join(format!("pnet-drive-token-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        let token = [7u8; 16];
        save_token(&path, &token).unwrap();
        assert_eq!(load_token(&path).unwrap(), Some(token));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_token_is_none() {
        let path = std::env::temp_dir().join(format!(
            "pnet-drive-missing-{}-{}",
            std::process::id(),
            "token"
        ));
        let _ = fs::remove_file(&path);
        assert_eq!(load_token(&path).unwrap(), None);
    }
}
