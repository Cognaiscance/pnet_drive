# pNet Drive

One user's files, kept by the server and copied into a folder on each of that user's devices. The web page lives on the server and is reached through the owner portal.

pNet carries the bytes. It does not store the files. `drive-client` and `drive-server` are different app aliases. The protocol is `application/drive`. Payloads start with a version byte and a type byte and stay inside one app datagram (under 4096 bytes; file bytes travel in chunks of at most 2048).

This is this user's own devices and that user's server. Sharing a folder with another pNet user, file previews, office editing, and selective sync of only some folders are out of scope.

## Roles

The holder is an own device running `drive-server`. A server-grade node wins over a device-grade node, then the lowest `sg_rank` (rank 0 sorts last, then the lower device id). Any other server stands by and does not open a second store. The client talks only to the holder. The holder accepts an approved `drive-client` on one of this user's own devices. A contact's client is ignored.

The server store is `~/.pnet/drive/` (`DRIVE_DIR` overrides). File bytes live under `blobs/` named by content hash. The tree (names, folders, hashes, sizes, and which device last wrote each path) is a separate record. The client's folder defaults to `~/pDrive` (`DRIVE_FOLDER` overrides). The client does not serve a page.

The page is loopback HTTP on `127.0.0.1:8822` (`DRIVE_HTTP_PORT`), mounted as `/apps/drive/` through `POST /api/app-web/register`. Push ports are `8820` for the client and `8821` for the server (`DRIVE_PUSH_PORT`).

## Rules that later steps keep

- A path is relative UTF-8 with `/` separators. `.` and `..` segments, an absolute path, and a NUL are rejected.
- An empty folder is a real entry.
- The server is the order of record. Two clients that change the same path in the same moment keep the change the server accepts first. The other client then downloads that copy.
- A delete moves the file or folder into trash with its original path and the time it was removed. Trash is visible on the web page. Restore puts it back at that path. If that path is already taken, restore is refused and the trash entry stays. Trash entries older than 48 hours are removed, including their blobs when nothing else still uses that hash.
- The same content hash is stored once. A client that already has the bytes does not upload them again.
- Retries of one transfer use the same message id and do not create a second file.

## Steps

Each step finishes on its own, on `develop`, with `cargo test` passing. Do not start a step until the one before it is on `develop`.

1. **Crate and roles.** Two binaries register with the local node and save a token. No files and no page. Done when both programs log that they registered and the token file is mode 0600.

2. **Server tree.** The holder opens the store. Tests cover a new folder, a file's name and hash, a nested path, a rejected `..` path, and reopening the same store after the process exits.

3. **List page.** The holder mounts `/apps/drive/`. The page lists one folder. Opening a subfolder shows only that folder's entries. An empty drive shows an empty folder, not an error.

4. **Web write.** From the page: upload a file, download it, create a folder, and delete a file. A download returns the bytes that were uploaded. These writes go straight into the store. Clients are not involved yet.

5. **Trash.** A delete from the page moves the entry to trash and records the original path. The trash view lists it. Restore puts it back. An entry older than 48 hours is gone on the next sweep. A blob still used by a live file is not deleted with the trash entry.

6. **Client folder.** The client creates its folder if needed and can list the local files with the same path rules as the server. It still does not talk about files.

7. **Watch and upload.** The client notices a new file, a changed file, a new folder, and a delete in its folder, and reports that to the holder. The holder's tree matches after the report. A change the client itself just wrote from the server is not reported back.

8. **Download.** A file that exists only on the server is written into the client folder with the same bytes and path. A folder is created the same way.

9. **Two clients.** One client adds a file. The other client's folder gains that file without a restart. A delete on one side moves the server copy to trash and removes it from the other folder. Restore on the page puts the file back in both folders.

## Check

From this directory:

```bash
cargo test
```
