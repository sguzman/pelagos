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
