//! The folder page served on loopback and mounted at `/apps/drive/`.
//!
//! Links are relative so they stay under `/apps/drive/` inside the portal frame.

use crate::store::{self, Entry, Kind, Store, StoreError, TrashItem};

const WEB_WRITER: [u8; 16] = [0u8; 16];

pub fn render(folder: &str, entries: &[Entry], note: Option<&str>) -> String {
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
    body.push_str("<p class=\"crumbs\"><a href=\"?trash=1\">Trash</a></p>\n");
    if let Some(note) = note {
        body.push_str("<p class=\"note\">");
        body.push_str(&esc(note));
        body.push_str("</p>\n");
    }
    body.push_str(&forms(folder));
    if entries.is_empty() {
        body.push_str("<p class=\"empty\">This folder is empty.</p>\n");
    } else {
        body.push_str("<ul class=\"files\">\n");
        for entry in entries {
            body.push_str("<li>");
            match entry.kind {
                Kind::Folder => {
                    body.push_str("<a href=\"?path=");
                    body.push_str(&url_enc(&entry.path));
                    body.push_str("\">");
                    body.push_str(&esc(entry.name()));
                    body.push_str("</a>");
                }
                Kind::File { size, .. } => {
                    body.push_str("<a href=\"file?path=");
                    body.push_str(&url_enc(&entry.path));
                    body.push_str("\">");
                    body.push_str(&esc(entry.name()));
                    body.push_str("</a>");
                    body.push_str(" <span class=\"size\">");
                    body.push_str(&size.to_string());
                    body.push_str(" bytes</span>");
                    body.push_str("<form method=\"post\"><input type=\"hidden\" name=\"action\" value=\"delete\"><input type=\"hidden\" name=\"path\" value=\"");
                    body.push_str(&esc(&entry.path));
                    body.push_str("\"><button type=\"submit\">Delete</button></form>");
                }
            }
            body.push_str("</li>\n");
        }
        body.push_str("</ul>\n");
    }
    body.push_str("</main>\n");
    body.push_str(THEME_SYNC);
    body.push_str("</body></html>\n");
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
    let mut html = String::from("<p class=\"crumbs\"><a href=\"?\">Drive</a>");
    if !folder.is_empty() {
        let mut built = String::new();
        for part in folder.split('/') {
            if !built.is_empty() {
                built.push('/');
            }
            built.push_str(part);
            html.push_str(" / <a href=\"?path=");
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

/// Repaint when the portal top bar writes `pnet-theme` in another document.
const THEME_SYNC: &str = r##"<script>
(function () {
  var key = "pnet-theme";
  function paint(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", theme === "light" ? "#f3eee4" : "#070605");
  }
  window.addEventListener("storage", function (ev) {
    if (ev.key === key && (ev.newValue === "light" || ev.newValue === "dark")) paint(ev.newValue);
  });
})();
</script>
"##;

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
.crumbs, .empty, .size, .note { color: var(--muted); }
form { display: flex; gap: 0.5rem; align-items: center; margin: 0.6rem 0; }
input, button {
  font: inherit; color: var(--ink); background: var(--bg-elev);
  border: 1px solid var(--line); border-radius: 8px; padding: 0.35rem 0.6rem;
}
ul.files { list-style: none; padding: 0; margin: 1rem 0; }
ul.files li { padding: 0.45rem 0; border-bottom: 1px solid var(--line); }
ul.files form { display: inline; margin-left: 0.6rem; }
"#;

pub fn handle(store: &mut Store, raw: &[u8]) -> Vec<u8> {
    let Some(req) = parse_request(raw) else {
        return http_response(400, "Bad Request", "<p>Bad request.</p>");
    };
    match req {
        Req::List { folder } => list_page(store, &folder, None),
        Req::Download { path } => match store.read_file(&path) {
            Ok(bytes) => file_response(&path, &bytes),
            Err(_) => list_page(store, "", Some("That file is not in the drive.")),
        },
        Req::Mkdir { folder, name } => {
            let back = folder.clone();
            match child_path(&folder, &name).and_then(|path| store.mkdir(&path, &WEB_WRITER)) {
                Ok(()) => redirect(&back),
                Err(err) => list_page(store, &back, Some(&err.to_string())),
            }
        }
        Req::Upload { folder, name, bytes } => {
            let back = folder.clone();
            match child_path(&folder, &name)
                .and_then(|path| store.put_file(&path, &bytes, &WEB_WRITER).map(|_| ()))
            {
                Ok(()) => redirect(&back),
                Err(err) => list_page(store, &back, Some(&err.to_string())),
            }
        }
        Req::Delete { path } => {
            let _ = store.sweep(store::now_ms());
            let back = parent_folder(&path);
            match store.trash_file(&path, store::now_ms()) {
                Ok(()) => redirect(&back),
                Err(err) => list_page(store, &back, Some(&err.to_string())),
            }
        }
        Req::Trash => trash_page(store, None),
        Req::Restore { path } => {
            let _ = store.sweep(store::now_ms());
            match store.restore(&path) {
                Ok(()) => redirect_trash(),
                Err(err) => trash_page(store, Some(&err.to_string())),
            }
        }
    }
}

fn trash_page(store: &Store, note: Option<&str>) -> Vec<u8> {
    http_response(200, "OK", &render_trash(store.trash(), note))
}

fn render_trash(items: &[TrashItem], note: Option<&str>) -> String {
    let mut body = String::from(
        "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"dark\"><head><meta charset=\"utf-8\">\
         <meta name=\"theme-color\" content=\"#070605\"><title>Trash</title>\n\
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
         </script><style>",
    );
    body.push_str(PAGE_CSS);
    body.push_str("</style></head><body><main>\n<h1>Trash</h1>\n<p class=\"crumbs\"><a href=\"?\">Drive</a></p>\n");
    if let Some(note) = note {
        body.push_str("<p class=\"note\">");
        body.push_str(&esc(note));
        body.push_str("</p>\n");
    }
    if items.is_empty() {
        body.push_str("<p class=\"empty\">Trash is empty.</p>\n");
    } else {
        body.push_str("<ul class=\"files\">\n");
        for item in items {
            body.push_str("<li>");
            body.push_str(&esc(&item.path));
            body.push_str("<form method=\"post\"><input type=\"hidden\" name=\"action\" value=\"restore\"><input type=\"hidden\" name=\"path\" value=\"");
            body.push_str(&esc(&item.path));
            body.push_str("\"><button type=\"submit\">Restore</button></form></li>\n");
        }
        body.push_str("</ul>\n");
    }
    body.push_str("<p class=\"empty\">Deleted files stay here for 2 days.</p>\n</main>\n");
    body.push_str(THEME_SYNC);
    body.push_str("</body></html>\n");
    body
}

fn redirect_trash() -> Vec<u8> {
    b"HTTP/1.1 303 See Other\r\nLocation: ?trash=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

fn list_page(store: &Store, folder: &str, note: Option<&str>) -> Vec<u8> {
    match store.list(folder) {
        Ok(entries) => http_response(200, "OK", &render(folder, &entries, note)),
        Err(StoreError::BadPath | StoreError::Missing | StoreError::NotAFolder) => http_response(
            404,
            "Not Found",
            &render(folder, &[], Some("That folder is not in the drive.")),
        ),
        Err(err) => http_response(500, "Internal Server Error", &format!("<p>{}</p>", esc(&err.to_string()))),
    }
}

fn forms(folder: &str) -> String {
    let folder = esc(folder);
    format!(
        "<form method=\"post\"><input type=\"hidden\" name=\"action\" value=\"mkdir\"><input type=\"hidden\" name=\"folder\" value=\"{folder}\"><input name=\"name\" required placeholder=\"Folder name\"><button type=\"submit\">New folder</button></form>\n\
         <form method=\"post\" enctype=\"multipart/form-data\"><input type=\"hidden\" name=\"action\" value=\"upload\"><input type=\"hidden\" name=\"folder\" value=\"{folder}\"><input type=\"file\" name=\"file\" required><button type=\"submit\">Upload</button></form>\n"
    )
}

fn redirect(folder: &str) -> Vec<u8> {
    let loc = if folder.is_empty() {
        "?".to_string()
    } else {
        format!("?path={}", url_enc(folder))
    };
    format!("HTTP/1.1 303 See Other\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

fn file_response(path: &str, bytes: &[u8]) -> Vec<u8> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename*=UTF-8''{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        url_enc(name),
        bytes.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(bytes);
    out
}

enum Req {
    List { folder: String },
    Trash,
    Download { path: String },
    Mkdir { folder: String, name: String },
    Upload { folder: String, name: String, bytes: Vec<u8> },
    Delete { path: String },
    Restore { path: String },
}

fn parse_request(raw: &[u8]) -> Option<Req> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let body = &raw[split + 4..];
    let mut lines = head.lines();
    let request = lines.next()?;
    let mut parts = request.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let content_type = header(head, "content-type").unwrap_or("");
    let expect = header(head, "content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if body.len() < expect {
        return None;
    }
    let body = &body[..expect];
    match (method, path) {
        ("GET", "/") if query_value(query, "trash").as_deref() == Some("1") => Some(Req::Trash),
        ("GET", "/") => Some(Req::List {
            folder: query_value(query, "path").unwrap_or_default(),
        }),
        ("GET", "/file") => Some(Req::Download {
            path: query_value(query, "path")?,
        }),
        ("POST", "/") => parse_post(content_type, body),
        _ => None,
    }
}

fn parse_post(content_type: &str, body: &[u8]) -> Option<Req> {
    if let Some(boundary) = content_type
        .split(';')
        .filter_map(|part| part.trim().strip_prefix("boundary="))
        .next()
    {
        let fields = parse_multipart(body, boundary.trim_matches('"'))?;
        let action = fields.iter().find(|f| f.name == "action")?.text();
        let folder = fields
            .iter()
            .find(|f| f.name == "folder")
            .map(|f| f.text())
            .unwrap_or_default();
        if action == "upload" {
            let file = fields.iter().find(|f| f.name == "file")?;
            let name = file.filename.clone()?;
            return Some(Req::Upload {
                folder,
                name,
                bytes: file.bytes.clone(),
            });
        }
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    let action = form_value(text, "action")?;
    match action.as_str() {
        "mkdir" => Some(Req::Mkdir {
            folder: form_value(text, "folder").unwrap_or_default(),
            name: form_value(text, "name")?,
        }),
        "delete" => Some(Req::Delete {
            path: form_value(text, "path")?,
        }),
        "restore" => Some(Req::Restore {
            path: form_value(text, "path")?,
        }),
        _ => None,
    }
}

struct Field {
    name: String,
    filename: Option<String>,
    bytes: Vec<u8>,
}

impl Field {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn parse_multipart(body: &[u8], boundary: &str) -> Option<Vec<Field>> {
    let mark = format!("--{boundary}").into_bytes();
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < body.len() {
        let start = body[pos..].windows(mark.len()).position(|w| w == mark.as_slice())? + pos;
        pos = start + mark.len();
        if body.get(pos..pos + 2) == Some(b"--") {
            break;
        }
        if body.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
        }
        let head_end = body[pos..].windows(4).position(|w| w == b"\r\n\r\n")? + pos;
        let head = std::str::from_utf8(&body[pos..head_end]).ok()?;
        pos = head_end + 4;
        let next = body[pos..].windows(mark.len()).position(|w| w == mark.as_slice())? + pos;
        let mut end = next;
        if end >= 2 && body[end - 2..end] == *b"\r\n" {
            end -= 2;
        }
        let (name, filename) = disposition(head)?;
        fields.push(Field {
            name,
            filename,
            bytes: body[pos..end].to_vec(),
        });
        pos = next;
    }
    Some(fields)
}

fn disposition(head: &str) -> Option<(String, Option<String>)> {
    let line = head.lines().find(|line| {
        line.to_ascii_lowercase().starts_with("content-disposition:")
    })?;
    let mut name = None;
    let mut filename = None;
    for part in line.split(';').skip(1) {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("name=") {
            name = Some(unquote(value));
        } else if let Some(value) = part.strip_prefix("filename=") {
            let value = unquote(value);
            filename = value.rsplit(['/', '\\']).next().map(str::to_string);
        }
    }
    Some((name?, filename))
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

fn query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(url_dec(v));
        }
    }
    None
}

fn form_value(body: &str, key: &str) -> Option<String> {
    query_value(body, key)
}

fn child_path(folder: &str, name: &str) -> Result<String, StoreError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(StoreError::BadPath);
    }
    let path = if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    };
    crate::store::check_path(&path)
}

fn parent_folder(path: &str) -> String {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string()).unwrap_or_default()
}

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
        let html = render("", &[], None);
        assert!(html.contains("This folder is empty."));
        assert!(html.contains("pnet-theme"));
        assert!(html.contains("data-theme=\"dark\""));
        assert!(html.contains("html[data-theme=\"light\"]"));
        assert!(!html.contains("theme-toggle"));
        assert!(!html.to_ascii_lowercase().contains("error"));
    }

    #[test]
    fn subfolder_lists_only_its_entries() {
        let html = render(
            "notes",
            &[folder("notes/2026"), file("notes/a.txt", 5)],
            None,
        );
        assert!(html.contains("a.txt"));
        assert!(html.contains("2026"));
        assert!(html.contains("href=\"?path=notes%2F2026\""));
        assert!(html.contains("href=\"file?path=notes%2Fa.txt\""));
        assert!(html.contains("New folder"));
        assert!(html.contains("Upload"));
        assert!(html.contains("Delete"));
        assert!(html.contains("Trash"));
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

    #[test]
    fn upload_download_mkdir_and_delete() {
        let dir = std::env::temp_dir().join(format!(
            "pnet-drive-page-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let mkdir = handle(
            &mut store,
            b"POST / HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 31\r\n\r\naction=mkdir&folder=&name=notes",
        );
        assert!(std::str::from_utf8(&mkdir).unwrap().starts_with("HTTP/1.1 303"));

        let body = b"hello drive";
        let upload = upload_request("notes", "a.txt", body);
        let uploaded = handle(&mut store, &upload);
        assert!(std::str::from_utf8(&uploaded).unwrap().starts_with("HTTP/1.1 303"));

        let downloaded = handle(
            &mut store,
            b"GET /file?path=notes%2Fa.txt HTTP/1.1\r\n\r\n",
        );
        let text = String::from_utf8_lossy(&downloaded);
        let raw = text.split("\r\n\r\n").nth(1).unwrap();
        assert_eq!(raw.as_bytes(), body);

        let removed = handle(
            &mut store,
            b"POST / HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 32\r\n\r\naction=delete&path=notes%2Fa.txt",
        );
        assert!(std::str::from_utf8(&removed).unwrap().contains("Location: ?path=notes"));
        assert!(store.get("notes/a.txt").unwrap().is_none());
        assert_eq!(store.trash()[0].path, "notes/a.txt");

        let listed = handle(&mut store, b"GET /?trash=1 HTTP/1.1\r\n\r\n");
        let listed = String::from_utf8_lossy(&listed);
        assert!(listed.contains("notes/a.txt"));
        assert!(listed.contains("Restore"));

        let restored = handle(
            &mut store,
            b"POST / HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 33\r\n\r\naction=restore&path=notes%2Fa.txt",
        );
        assert!(std::str::from_utf8(&restored).unwrap().contains("Location: ?trash=1"));
        assert_eq!(store.read_file("notes/a.txt").unwrap(), body);
        assert!(store.trash().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn upload_request(folder: &str, name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"--bound\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\nupload\r\n");
        body.extend_from_slice(b"--bound\r\nContent-Disposition: form-data; name=\"folder\"\r\n\r\n");
        body.extend_from_slice(folder.as_bytes());
        body.extend_from_slice(b"\r\n--bound\r\nContent-Disposition: form-data; name=\"file\"; filename=\"");
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(b"\"\r\nContent-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n--bound--\r\n");
        let mut req = format!(
            "POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=bound\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(&body);
        req
    }
}
