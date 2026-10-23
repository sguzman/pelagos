# Pelagos 🌊🦀

**The open sea, under your control.** A Rust-native replacement for Jellyfin Server, designed for compatibility with existing Jellyfin clients, including the **unmodified Smart TV Jellyfin apps**.

![Pelagos ocean-turquoise project color](assets/pelagos-color.svg)

**Identity:** `#1DB2C0` · `oklch(70% 0.115 205)` ([color decision](docs/COLOR.md)). **Administration appearance:** ◐ System (default) / ☀ Light / ☾ Dark, with the selector at the top right.

> **Pre-alpha, not yet Jellyfin compatible.** Rust login with persistent device-aware session records, scoped `/Sessions` queries, item browsing, virtual folders, read-only scanning, FFprobe and SQLite indexing now exist. FFprobe-backed read-only PlaybackInfo, catalog-v2 persisted empty libraries, permission-scoped Search/Hints, and static audio/video byte-range streaming are synthetic-test verified. Users/Items responses are incomplete, ordinary accounts require explicit local per-library grants, and there is still no client-accepted authentication, Quick Connect, remux/transcoding, user-data migration or Smart TV playback. Library-authorized **static** video streaming with HTTP byte ranges is available for synthetic test media. Full compatibility is a mandatory acceptance goal, not a current claim.

## 🎯 MVP means the COMPLETE Jellyfin Server port

- Replace all built-in Jellyfin Server v12.3 C#/.NET application logic with Rust, independently implemented with protocol and behavioral parity. **No .NET runtime or original Jellyfin daemon.**
- Connect **existing, unchanged** Jellyfin Smart TV clients: Android TV, Fire TV, LG webOS, Samsung Tizen, and Roku, plus web and mobile clients.
- Match core library, metadata, search, user/auth, playback, transcode, subtitles, SyncPlay, Live TV, admin, tasks, backup, settings and important first-party features.
- Retain `ffmpeg`, `ffprobe`, Linux filesystems and GPU drivers instead of pointlessly rewriting proven media tools.
- Safely import compatible existing library/user data without changing the user's original Jellyfin installation.

**Full-port acceptance:** all [M0–M6 milestones](docs/ROADMAP.md) must be met. Passing first TV playback is a major gate but **not MVP completion**.

## 📊 Development status

`██░░░░░░░░ 15%` across 96 atomic tasks (accepted-check arithmetic). This is project completion accounting, **not a claim of passing CI or Smart TV operation**. Every task has evidence-gated checks. See [machine-readable ledger](docs/progress.json), [full roadmap](docs/ROADMAP.md), [source baseline](docs/SOURCE_BASELINE.md), [port matrix](docs/PORT_MATRIX.md) and [TV compatibility contract](docs/COMPATIBILITY.md).

**Latest passing CI:** [run 39968255232](https://github.com/sguzman/pelagos/actions/runs/39968255232) — formatting, Clippy, Rust item-filter/search/audio/authorization tests and pinned source-matrix validation.

## 🚀 Current starter

```sh
cargo run -- --bind 127.0.0.1 --port 8097
# Read-only offline inventory; no server started, no writes to media:
cargo run -- scan /path/to/authorized/media
# Optional FFprobe metadata inspection (requires the system's ffprobe):
cargo run -- probe /path/to/authorized/media.mp4
# Populate the separate Pelagos SQLite catalog without changing source media:
cargo run -- index /path/to/authorized/media
cargo run -- catalog /path/to/authorized/media
# CLI account bootstrap: user-add reads a password securely from standard input.
# Supply it using a trusted password source piped to:
# cargo run -- user-add admin
# Grant/revoke access to an explicitly indexed media root for a local account:
cargo run -- library-grant viewer /path/to/authorized/media
cargo run -- library-revoke viewer /path/to/authorized/media
# Opt-in LAN discovery (does NOT enable login or TV streaming):
cargo run -- --bind 0.0.0.0 --port 8097 --discover
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Open `http://127.0.0.1:8097/web/`. JSON diagnostics: `/health`, `/api/pelagos/status`. The partially authenticated prototype deliberately defaults to **loopback-only** and port 8097, avoiding the usual Jellyfin port 8096. UDP discovery is opt-in, on port 7359, only responds to LAN/loopback peers and cannot run alongside another service already bound to that UDP port. Do not expose this build to untrusted networks. Requires a normal Rust toolchain; no Python tooling is required.

The provisional HTTP routes include `GET /Search/Hints`, `GET/HEAD /Audio/{id}/stream?static=true`, and `POST /Users/AuthenticateByName`, `GET /Users/Me`, `POST /Sessions/Logout`, `GET /UserViews`, `GET /Items`, `GET /Items/{id}` and `GET /Users/{user_id}/Items`. The catalog and search routes support recognized type/media filters and require authentication; administrators can see all indexed libraries, while non-administrators have default-deny access with explicit root grants. A user with library access may request `GET /Videos/{id}/stream?static=true` or `HEAD` for a previously indexed, unchanged video file, including a single HTTP `Range` header. Non-static/transcoding requests explicitly return 501. Audio and video static direct play share the same authorization, byte-range and checked-file implementation. On Linux the opened file descriptor is checked against its canonical indexed root before media bytes are sent, guarding the tested symlink-swap race. This is a narrow synthetic-tested direct-file stream, not proof an unmodified TV client works.

## 🦀 Architecture and independence

Rust owns HTTP, protocols, indexing, metadata, state, playback negotiations, and FFmpeg supervision. A lightweight browser admin shell is included as a scaffold. The system supports Linux-first development, with EndeavourOS/Hyprland a primary target. The existing user-installed Jellyfin server remains independent and untouched.

[Architecture](docs/ARCHITECTURE.md) · [Pinned API attributes](docs/API_CONTRACTS.md) · [Compatibility](docs/COMPATIBILITY.md) · [Provenance and licensing](docs/PROVENANCE.md) · [Tracking](docs/ISSUES.md).

Pelagos is independent of and not endorsed by Jellyfin. This repository retains its original MIT license for independently authored implementation; Jellyfin's GPL-2.0 source must not be copied/translated without meeting derivative-work licensing obligations.
