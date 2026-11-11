//! Registers as drive-client and keeps a local folder. It does not sync yet.

use std::net::SocketAddr;
use std::thread;
use std::time::Duration;

use pnet_drive::local;
use pnet_drive::node::{self, NodeClient};
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
    match node.register(CLIENT_ALIAS, push_port) {
        Ok(token) => {
            if let Err(err) = node::save_token(&token_path, &token) {
                log::error!("[drive-client] token file: {err}");
            }
            log::info!("[drive-client] registered as {CLIENT_ALIAS}");
        }
        Err(err) => {
            log::error!("[drive-client] register failed: {err}");
            std::process::exit(1);
        }
    }

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

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
