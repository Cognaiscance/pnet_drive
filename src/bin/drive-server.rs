//! Registers as drive-server, opens the file store, and lists folders on the portal.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pnet_drive::node::{self, NodeClient};
use pnet_drive::page::{self, http_response};
use pnet_drive::store::{Store, StoreError};
use pnet_drive::{
    DEFAULT_HTTP_PORT, DEFAULT_SERVER_PUSH, SERVER_ALIAS, WEB_SLUG, WEB_TITLE,
};

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
    let http_port = env_or("DRIVE_HTTP_PORT", &DEFAULT_HTTP_PORT.to_string())
        .parse()
        .unwrap_or(DEFAULT_HTTP_PORT);
    let portal = env_or("PNET_PORTAL", "http://127.0.0.1:8777");

    let _push = node::bind_push(push_port).unwrap_or_else(|err| {
        log::error!("[drive-server] bind 127.0.0.1:{push_port}: {err}");
        std::process::exit(1);
    });
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

    let dir = node::data_dir();
    let store = match Store::open(&dir) {
        Ok(store) => Arc::new(Mutex::new(store)),
        Err(err) => {
            log::error!("[drive-server] store {}: {err}", dir.display());
            std::process::exit(1);
        }
    };
    log::info!("[drive-server] store {}", dir.display());

    let listener = match TcpListener::bind(("127.0.0.1", http_port)) {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("[drive-server] http 127.0.0.1:{http_port}: {err}");
            std::process::exit(1);
        }
    };
    listener.set_nonblocking(true).ok();
    let page_store = Arc::clone(&store);
    thread::spawn(move || http_loop(listener, page_store));

    for attempt in 1..=5 {
        match portal_register(&portal, http_port) {
            Ok(()) => {
                log::info!("[drive-server] portal /apps/{WEB_SLUG}/ → 127.0.0.1:{http_port}");
                break;
            }
            Err(err) => {
                log::error!("[drive-server] portal register attempt {attempt}: {err}");
                if attempt == 5 {
                    log::error!("[drive-server] the page is on 127.0.0.1:{http_port} only");
                }
                thread::sleep(Duration::from_millis(400));
            }
        }
    }

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn http_loop(listener: TcpListener, store: Arc<Mutex<Store>>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    if let Err(err) = serve_http(stream, &store) {
                        log::error!("[drive-server] http: {err}");
                    }
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                log::error!("[drive-server] accept: {err}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn serve_http(mut stream: TcpStream, store: &Mutex<Store>) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).map_err(|err| err.to_string())?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");
    let Some(folder) = page::folder_query(line).unwrap_or(None) else {
        let resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(resp).ok();
        return Ok(());
    };
    let listed = store
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .list(&folder);
    let (status, reason, html) = match listed {
        Ok(entries) => (200, "OK", page::render(&folder, &entries)),
        Err(StoreError::BadPath | StoreError::Missing | StoreError::NotAFolder) => (
            404,
            "Not Found",
            page::render(&folder, &[]).replace(
                "This folder is empty.",
                "That folder is not in the drive.",
            ),
        ),
        Err(err) => (
            500,
            "Internal Server Error",
            format!("<p>{}</p>", esc(&err.to_string())),
        ),
    };
    stream
        .write_all(&http_response(status, reason, &html))
        .map_err(|err| err.to_string())
}

fn portal_register(portal: &str, port: u16) -> Result<(), String> {
    let body = format!("slug={WEB_SLUG}&port={port}&title={}", form_enc(WEB_TITLE));
    http_post(portal, "/api/app-web/register", &body)
}

fn http_post(base: &str, path: &str, body: &str) -> Result<(), String> {
    let base = base.trim_end_matches('/');
    let (host, port) = parse_http(base)?;
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .map_err(|err| err.to_string())?;
    if resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(resp.lines().next().unwrap_or("portal error").to_string())
    }
}

fn parse_http(url: &str) -> Result<(String, u16), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "portal URL must be http://".to_string())?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    if let Some((host, port)) = hostport.rsplit_once(':') {
        let port = port.parse().map_err(|_| "bad portal port".to_string())?;
        Ok((host.to_string(), port))
    } else {
        Ok((hostport.to_string(), 80))
    }
}

fn form_enc(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
