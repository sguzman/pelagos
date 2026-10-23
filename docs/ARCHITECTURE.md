# 🦀 Architecture

## Boundary

```text
Unmodified Jellyfin apps (TV, browser, mobile)
   ├── HTTP REST + WebSockets + discovery
   ├── streaming URLs, HLS and client playback negotiation
   └── session/device auth + Quick Connect
                      ↕
          PELAGOS (Rust; no .NET)
   ├── API compatibility and authentication
   ├── domain models, library management, metadata
   ├── playback decision engine, streams and transcoding manager
   ├── tasks, plugins/replacement capabilities, logging
   ├── persistent SQLite + caches + media source indexes
   └── administration web UI (ultimately Rust/WASM-capable)
                      ↕
         Linux filesystem / FFprobe / FFmpeg / GPU drivers
```

No C# service, embedded .NET runtime, or proxied Jellyfin daemon may be required for MVP. Use mature Linux tools for probing/transcoding; Rust owns process supervision and compatibility decisions. The starter admin UI uses small HTML/CSS/JS assets served by Rust; future Rust/WASM is permitted, and no C# is involved.

## Bootstrap (not an accepted client protocol)

`cargo run -- --bind 127.0.0.1 --port 8097` provides `/health`, `/api/pelagos/status`, `/web/` and a provisional `/System/Info/Public`. No authentication, media library, scanning or actual Jellyfin TV playback is implemented. Status JSON explicitly reports `jellyfin_compatible: false`. A shape-compatible endpoint is not an acceptance test.

The default listener is loopback-only on port 8097, deliberately avoiding collision with Jellyfin's 8096. Do **not** expose this pre-alpha service to a public network; LAN access is a later milestone once auth and disclosure policies are validated. Persistent identity is in XDG state at `$XDG_STATE_HOME/pelagos/server-id` or `~/.local/state/pelagos/server-id`.

## Constraints

Prefer a single Rust package with clearly separated modules until concrete scaling needs justify multiple crates. Keep media roots configurable, never mutate original media without explicit operation and tested backup behavior. Build an isolated Pelagos database; any Jellyfin data import is read-only against source data and previewed first. Retain FFmpeg/FFprobe and hardware acceleration without codec reinvention. GUI appearance: upper-right System (default), Light, Dark, persisted locally, with live OS theme following when System is selected.

## Read-only diagnostics

The Rust-only `pelagos scan <directory>` subcommand enumerates recognized file extensions, sizes and relative paths, sorted deterministically. It never writes source media, never follows file or directory symlinks inside the library root, does not run Jellyfin, and does not start HTTP. This is a testable inventory diagnostic only: no persistent indexing, metadata inference, user library, remote client delivery or FFprobe analysis is claimed yet. Synthetic fixtures and a symlink boundary test run in GitHub CI.

## Initial read-only FFprobe bridge

`cargo run -- probe /path/to/file.mkv` invokes system `ffprobe` with explicit argv, a process timeout, bounded output and JSON decoding. It never runs a shell or transcodes media. Synthetic tests verify error paths; real hardware/audio/video metadata mapping and Jellyfin `MediaStream` compatibility are still open. This does not make playback functional.

## Private incremental media index

`cargo run -- index /media/root` scans an explicitly provided media directory without following symlinks and writes only to Pelagos' own XDG-state `pelagos.sqlite3` database. The schema is versioned (`user_version = 1`), entries are scoped by canonical library root, indexes commit atomically, and stale-row purging happens only after the entire scan succeeds. `cargo run -- catalog /media/root` lists indexed entries without touching media. This is a preliminary filename/type catalog, not Jellyfin's rich media object database or a TV-accessible library.

## LAN service discovery

`--bind 0.0.0.0 --discover` explicitly enables the v12.3 Jellyfin UDP discovery phrase and `ServerDiscoveryInfo` JSON reply on UDP port 7359. System chooses a route-specific local interface IP for the reply and ignores non-LAN source addresses. The default remains loopback-only and discovery-disabled. A discovery packet or client visibility is NOT authentication, browsing or media playback; these require further server implementation. Do not expose pre-alpha HTTP admin surfaces to untrusted networks.

## Authenticated SQLite catalog API (pre-alpha)

`pelagos user-add USERNAME` reads a passphrase from standard input (not command-line arguments), applies Argon2id and stores a separate `pelagos-auth.sqlite3` database with SHA-256 digests of randomly generated API tokens. The first account is an administrator, subsequent accounts are non-admin. The initial `/Users/AuthenticateByName` route and `X-Emby-Token` / `MediaBrowser Token=` authorization header handling are backed by synthetic HTTP tests. User policy, revocation beyond single logout, session metadata, Quick Connect, rate limiting, and unmodified client acceptance remain open; this prototype must not be exposed to untrusted networks.

The read-only `/Items`, `/Items/{id}` and `/UserViews` endpoints map private SQLite filenames into stable IDs, synthetic virtual folders and `ParentId` relationships; `parentId` and `recursive` queries can navigate arbitrary synthetic nested directories. These are partial DTOs, deliberately conceal absolute library paths, and require admin privileges. They do **not** yet supply provider metadata, art, Jellyfin-compatible episodes/seasons, normal user access, media streams or playback.

## Initial static file streaming

The `/Videos/{itemId}/stream?static=true` GET and HEAD endpoints map a previously indexed video into an async byte stream, support single RFC 7233-style byte range / suffix ranges, respond with appropriate 206 and 416 headers, and require an administrator-authenticated session. After resolving the indexed file, the handler canonicalizes its root and file path and rejects traversal outside the root. [CI 39958988196](https://github.com/sguzman/pelagos/actions/runs/39958988196) uses synthetic byte fixtures and symlink-replacement cases. This is not yet a hardened openat2-based filesystem sandbox and does not cover remuxing/transcoding, arbitrary TV codec negotiation, zero-copy networking, subtitles, or the Jellyfin PlaybackInfo negotiation contract. Keep the service private during pre-alpha.
