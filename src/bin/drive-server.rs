//! Registers as drive-server and waits. The file store and the page come later.

use std::net::SocketAddr;
use std::thread;
use std::time::Duration;

use pnet_drive::node::{self, NodeClient};
use pnet_drive::{DEFAULT_SERVER_PUSH, SERVER_ALIAS};

fn main() {
    pnet_drive::log_setup::init("drive-server");
    let addr = env_or("PNET_ADDR", "127.0.0.1:7777");
    let addr: SocketAddr = addr.parse().unwrap_or_else(|_| {
        log::error!("[drive-server] PNET_ADDR is not host:port");
        std::process::exit(1);
    });
    let push_port = env_or("DRIVE_PUSH_PORT", &DEFAULT_SERVER_PUSH.to_string())
        .parse()
        .unwrap_or(DEFAULT_SERVER_PUSH);

    if let Err(err) = node::bind_push(push_port) {
        log::error!("[drive-server] bind 127.0.0.1:{push_port}: {err}");
        std::process::exit(1);
    }
    let node = NodeClient::connect(addr).unwrap_or_else(|err| {
        log::error!("[drive-server] local node {addr}: {err}");
        std::process::exit(1);
    });
    let token_path = node::token_path("drive-server");
    match node.register(SERVER_ALIAS, push_port) {
        Ok(token) => {
            if let Err(err) = node::save_token(&token_path, &token) {
                log::error!("[drive-server] token file: {err}");
            }
            log::info!("[drive-server] registered as {SERVER_ALIAS}");
        }
        Err(err) => {
            log::error!("[drive-server] register failed: {err}");
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
