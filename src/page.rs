//! The folder list served on loopback and mounted at `/apps/drive/`.

use crate::store::{Entry, Kind};

pub fn render(folder: &str, entries: &[Entry]) -> String {
    let mut body = String::from(
        "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"dark\"><head><meta charset=\"utf-8\">\
         <meta name=\"theme-color\" content=\"#070605\"><title>Drive</title>\n\
         <script>\
         (function () {\
           var theme = \"dark\";\
           try {\
             var saved = localStorage.getItem(\"pnet-theme\");\
             if (saved === \"light\" || saved === \"dark\") theme = saved;\
           } catch (e) {}\
           document.documentElement.setAttribute(\"data-theme\", theme);\
           var meta = document.querySelector('meta[name=\"theme-color\"]');\
           if (meta) meta.setAttribute(\"content\", theme === \"light\" ? \"#f3eee4\" : \"#070605\");\
         })();\
         </script>\n<style>",
    );
    body.push_str(PAGE_CSS);
    body.push_str("</style></head><body>\n<main>\n<h1>Drive</h1>\n");
    body.push_str(&crumbs(folder));
    if entries.is_empty() {
        body.push_str("<p class=\"empty\">This folder is empty.</p>\n");
    } else {
        body.push_str("<ul class=\"files\">\n");
        for entry in entries {
            body.push_str("<li>");
            match entry.kind {
                Kind::Folder => {
                    body.push_str("<a href=\"/?path=");
                    body.push_str(&url_enc(&entry.path));
                    body.push_str("\">");
                    body.push_str(&esc(entry.name()));
                    body.push_str("</a>");
                }
                Kind::File { size, .. } => {
                    body.push_str(&esc(entry.name()));
                    body.push_str(" <span class=\"size\">");
                    body.push_str(&size.to_string());
                    body.push_str(" bytes</span>");
                }
            }
            body.push_str("</li>\n");
        }
        body.push_str("</ul>\n");
    }
    body.push_str("</main></body></html>\n");
    body
}

pub fn http_response(status: u16, reason: &str, html: &str) -> Vec<u8> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        html.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(html.as_bytes());
    out
}

/// The folder named by `path` on `GET /`. Anything else is not the list page.
pub fn folder_query(request_line: &str) -> Result<Option<String>, ()> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(())?;
    let target = parts.next().ok_or(())?;
    if method != "GET" {
        return Err(());
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/" {
        return Ok(None);
    }
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "path" {
            return Ok(Some(url_dec(value)));
        }
    }
    Ok(Some(String::new()))
}

fn crumbs(folder: &str) -> String {
    let mut html = String::from("<p class=\"crumbs\"><a href=\"/\">Drive</a>");
    if !folder.is_empty() {
        let mut built = String::new();
        for part in folder.split('/') {
            if !built.is_empty() {
                built.push('/');
            }
            built.push_str(part);
            html.push_str(" / <a href=\"/?path=");
            html.push_str(&url_enc(&built));
            html.push_str("\">");
            html.push_str(&esc(part));
            html.push_str("</a>");
        }
    }
    html.push_str("</p>\n");
    html
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn url_enc(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn url_dec(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

const PAGE_CSS: &str = r#"
:root, html[data-theme="dark"] {
  color-scheme: dark;
  --bg: #070605; --bg-glow: #2a2218; --bg-elev: #14110d;
  --ink: #f4efe6; --muted: #b7aa96; --gold: #d4b15a;
  --line: rgba(244, 239, 230, 0.12);
}
html[data-theme="light"] {
  color-scheme: light;
  --bg: #f3eee4; --bg-glow: #f0e0be; --bg-elev: #fffdf8;
  --ink: #241c14; --muted: #6b5844; --gold: #8d6818;
  --line: rgba(36, 28, 20, 0.1);
}
* { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; }
body {
  background: radial-gradient(1100px 520px at 50% -12%, var(--bg-glow) 0%, transparent 58%), var(--bg);
  color: var(--ink);
  font-family: "Segoe UI", system-ui, -apple-system, sans-serif;
  line-height: 1.5;
}
h1 { font-family: Georgia, "Iowan Old Style", Palatino, serif; font-weight: 600; margin: 0 0 0.6rem; }
main { max-width: 40rem; margin: 0 auto; padding: 2rem 1.2rem 3rem; }
a { color: var(--gold); text-decoration: none; }
a:hover { text-decoration: underline; }
.crumbs, .empty, .size { color: var(--muted); }
ul.files { list-style: none; padding: 0; margin: 1rem 0; }
ul.files li { padding: 0.45rem 0; border-bottom: 1px solid var(--line); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> Entry {
        Entry {
            path: path.into(),
            kind: Kind::File {
                hash: [0u8; 32],
                size,
            },
            writer: [0u8; 16],
        }
    }

    fn folder(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: Kind::Folder,
            writer: [0u8; 16],
        }
    }

    #[test]
    fn empty_folder_is_not_an_error() {
        let html = render("", &[]);
        assert!(html.contains("This folder is empty."));
        assert!(!html.to_ascii_lowercase().contains("error"));
    }

    #[test]
    fn subfolder_lists_only_its_entries() {
        let html = render(
            "notes",
            &[folder("notes/2026"), file("notes/a.txt", 5)],
        );
        assert!(html.contains("a.txt"));
        assert!(html.contains("2026"));
        assert!(html.contains("href=\"/?path=notes%2F2026\""));
        assert!(!html.contains("plan.txt"));
    }

    #[test]
    fn query_keeps_the_folder_and_ignores_raw() {
        assert_eq!(
            folder_query("GET /?raw=1 HTTP/1.1").unwrap(),
            Some(String::new())
        );
        assert_eq!(
            folder_query("GET /?raw=1&path=notes%2F2026 HTTP/1.1").unwrap(),
            Some("notes/2026".into())
        );
        assert_eq!(folder_query("GET /favicon.ico HTTP/1.1").unwrap(), None);
    }
}
