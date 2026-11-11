//! Registers as drive-client, watches its folder, and reports changes to the holder.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::thread;
use std::time::Duration;

use pnet_drive::directory;
use pnet_drive::local;
use pnet_drive::node::{self, NodeClient};
use pnet_drive::sync::{self, Change, Watch};
use pnet_drive::{CLIENT_ALIAS, DEFAULT_CLIENT_PUSH};

fn main() {
    pnet_drive::log_setup::init("drive-client");
    let addr = env_or("PNET_ADDR", "127.0.0.1:7777");
    let addr: SocketAddr = addr.parse().unwrap_or_else(|_| {
        log::error!("[drive-client] PNET_ADDR is not host:port");
        std::process::exit(1);
    });
    let push_port = env_or("DRIVE_PUSH_PORT", &DEFAULT_CLIENT_PUSH.to_string())
        .parse()
        .unwrap_or(DEFAULT_CLIENT_PUSH);

    let _push = node::bind_push(push_port).unwrap_or_else(|err| {
        log::error!("[drive-client] bind 127.0.0.1:{push_port}: {err}");
        std::process::exit(1);
    });
    let node = NodeClient::connect(addr).unwrap_or_else(|err| {
        log::error!("[drive-client] local node {addr}: {err}");
        std::process::exit(1);
    });
    let token_path = node::token_path("drive-client");
    let token = match node.register(CLIENT_ALIAS, push_port) {
        Ok(token) => {
            if let Err(err) = node::save_token(&token_path, &token) {
                log::error!("[drive-client] token file: {err}");
            }
            log::info!("[drive-client] registered as {CLIENT_ALIAS}");
            token
        }
        Err(err) => {
            log::error!("[drive-client] register failed: {err}");
            std::process::exit(1);
        }
    };

    let folder = local::folder_dir();
    if let Err(err) = local::ensure(&folder) {
        log::error!("[drive-client] folder {}: {err}", folder.display());
        std::process::exit(1);
    }
    match local::list(&folder) {
        Ok(entries) => {
            log::info!(
                "[drive-client] folder {} ({} entries)",
                folder.display(),
                entries.len()
            );
        }
        Err(err) => {
            log::error!("[drive-client] folder {}: {err}", folder.display());
            std::process::exit(1);
        }
    }

    let mut watch = Watch::new();
    if let Err(err) = watch.poll(&folder) {
        log::error!("[drive-client] folder {}: {err}", folder.display());
        std::process::exit(1);
    }
    let mut sent: HashSet<[u8; 32]> = HashSet::new();

    loop {
        thread::sleep(Duration::from_secs(2));
        let changes = match watch.poll(&folder) {
            Ok(changes) => changes,
            Err(err) => {
                log::error!("[drive-client] folder {}: {err}", folder.display());
                continue;
            }
        };
        if changes.is_empty() {
            watch.commit();
            continue;
        }
        let holder = match node.get_data(&token).and_then(|reply| {
            directory::parse_get_data(&reply).map_err(|_| pnet_drive::node::NodeError::Unexpected)
        }) {
            Ok(dir) => dir,
            Err(err) => {
                log::error!("[drive-client] directory: {err}");
                continue;
            }
        };
        let Some(peer) = directory::report_server(&holder) else {
            log::error!("[drive-client] no drive server is approved yet");
            continue;
        };
        let mut failed = false;
        for change in &changes {
            let have = match change {
                Change::File { bytes, .. } => sent.contains(&sync::content_hash(bytes)),
                _ => false,
            };
            let mut sent_ok = true;
            for packet in sync::encode(change, have) {
                if let Err(err) = node.send(&token, &peer.device, &peer.app, &packet) {
                    log::error!("[drive-client] send: {err}");
                    failed = true;
                    sent_ok = false;
                }
            }
            if sent_ok {
                if let Change::File { bytes, .. } = change {
                    sent.insert(sync::content_hash(bytes));
                }
            }
            log::info!("[drive-client] reported a change");
        }
        if !failed {
            watch.commit();
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
