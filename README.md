# pnet_drive

A file drive for one pNet user. The server keeps the files and the web page. Each client keeps a copy in a folder on that machine and sends changes to the server.

pNet only carries the bytes. The two programs register as different apps and share the protocol `application/drive`.

| Program | Where it runs | What it does |
|---------|----------------|--------------|
| `drive-client` | Every device that should keep a copy | Watches `~/Drive`. It reports local changes, writes files that exist only on the server, and removes a file the server moved to trash. A restore on the page comes back down to each client. |
| `drive-server` | Server-grade nodes | Opens the file store and mounts `/apps/drive/`. The page lists a folder and can upload, download, create a folder, and delete a file. Deleted files stay in Trash for 2 days. |

The steps are in [descriptions/action-plan.md](descriptions/action-plan.md).

## Run

A local pNet node must already be listening. Approve both apps on the portal after the first start. Sign in and open **Drive** (`/apps/drive/`).

```bash
cargo run --bin drive-server
cargo run --bin drive-client
```

## Environment

| Variable | Default | Meaning |
|----------|---------|---------|
| `PNET_ADDR` | `127.0.0.1:7777` | Local pNet node |
| `PNET_PORTAL` | `http://127.0.0.1:8777` | Portal, used by the server to register the page |
| `DRIVE_HTTP_PORT` | `8822` | Loopback page on the server |
| `DRIVE_PUSH_PORT` | `8820` client, `8821` server | Loopback port where this process receives app pushes |
| `DRIVE_TOKEN_FILE` | `~/.pnet/drive-client/token` or `drive-server` | Saved app token. Each alias has its own directory. |
| `DRIVE_DIR` | `~/.pnet/drive` | Server store. The tree is `tree`. File bytes are `blobs/` named by SHA-256. |
| `DRIVE_FOLDER` | `~/Drive` | Folder the client keeps on this machine. |

## Check

```bash
cargo test
```
