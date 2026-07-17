# update-server-api

A small API that fronts update server object storage.

It replaces the static filestore that used to serve `update.rwfc.net`. The zips
now live in object storage behind `https://cdn.update.rwfc.net`; this service
serves only the three text files the updaters read, rendering them on demand
from a single JSON manifest.

## Endpoints

| Route | Serves |
| --- | --- |
| `GET /RetroRewind/RetroRewindVersion.txt` | `<version> <url> <path> <description>`, one line per update zip |
| `GET /RetroRewind/RetroRewindDelete.txt` | `<version> <path>`, one line per deleted file |
| `GET /RetroRewind/RetroRewindInstall.txt` | URL of the newest full download |
| `GET /RetroRewind/zip/RetroRewind.zip` | The legacy reinstall zip, if `legacy_reinstall_zip` is set |

Admin routes need `Authorization: Bearer <admin_token>`:

| Route | Does |
| --- | --- |
| `GET /admin/manifest` | Returns the manifest as JSON |
| `PUT /admin/manifest` | Replaces the whole manifest |
| `POST /admin/versions` | Appends one release |

Writes are validated, then written to a temp file and renamed, then swapped in.
If validation or the write fails, the served files are left untouched.

```sh
curl -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -X POST https://update.rwfc.net/admin/versions -d '{
    "version": "6.12.0",
    "zips": ["https://cdn.update.rwfc.net/RetroRewind/zip/6.12.0.zip"],
    "deletes": ["/6.12.0.zip"],
    "full_download": "https://cdn.update.rwfc.net/RetroRewind/zip/RetroRewind-6.12.0.zip"
  }'
```

## Manifest

`manifest.json` is the source of truth: an ordered array of releases, ascending
by version.

```json
{
  "versions": [
    {
      "version": "4.0.0",
      "zips": ["https://cdn.update.rwfc.net/RetroRewind/zip/1000.zip"],
      "deletes": ["/RetroRewind6/strm/0_F.brstm"],
      "full_download": "https://cdn.update.rwfc.net/RetroRewind/zip/RetroRewind-4.0.0.zip"
    }
  ]
}
```

`zips` and `deletes` are plural because the real data needs both: 4.0.0 ships
two zips (`1000.zip` and `1000Music.zip`), while 3.7.1 and 4.0.1 delete files
without shipping a zip, so they appear in `RetroRewindDelete.txt` but never in
`RetroRewindVersion.txt`. A release with no zips renders no version line.

`RetroRewindVersion.txt` has two columns that are rendered rather than stored,
since the updaters still parse four: the install path is `/` + the URL's
filename, and the description is always `Assets`.

`full_download` is optional and set on releases that cut a full install zip.
`RetroRewindInstall.txt` serves the newest one. Because object storage is
cached, each full download needs its own filename rather than reusing
`RetroRewind.zip`.

Validation rejects non-numeric versions, versions that aren't strictly
ascending, URLs that don't end in a filename, and any field containing
whitespace (both text formats are whitespace-separated, so a stray space would
corrupt the line).

## Legacy reinstall zip

Old PC clients reinstall from a fixed `/RetroRewind/zip/RetroRewind.zip` rather
than reading `RetroRewindInstall.txt`. Setting `legacy_reinstall_zip` to a file
on the host serves it at that path, streamed and with range requests so large
downloads resume. Everything else about the migration is unaffected by it.

This is temporary. Once enough of the playerbase has moved to clients that read
`RetroRewindInstall.txt`, comment the key out and the route disappears.

If the key is set but the file is missing, the server logs a warning, keeps
serving everything else, and the route 404s — a broken shim never takes the
update files down with it.

## Running

```sh
cp config.example.toml config.toml   # gitignored; holds the admin token
cargo run                            # or: cargo run -- /path/to/config.toml
```

## Tests

```sh
cargo test
```

The manifest was migrated from the old static filestore's `RetroRewindVersion.txt`
and `RetroRewindDelete.txt`, and rendering it reproduced both byte-for-byte apart
from the CDN URL swap and one deliberate fix: 6.6.0 served `6.6.zip` but declared
its install path as `/6.6.0.zip`, while its delete entry was `/6.6.zip`, so the
downloaded zip was never cleaned up. Deriving the path from the URL corrects
that.
