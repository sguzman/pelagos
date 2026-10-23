# Pelagos 🌊🦀

**The open sea, under your control.** A Rust-native replacement for Jellyfin Server, designed for compatibility with existing Jellyfin clients, including the **unmodified Smart TV Jellyfin apps**.

![Pelagos ocean-turquoise project color](assets/pelagos-color.svg)

**Identity:** `#1DB2C0` · `oklch(70% 0.115 205)` ([color decision](docs/COLOR.md)). **Administration appearance:** ◐ System (default) / ☀ Light / ☾ Dark, with the selector at the top right.

> **Pre-alpha scaffolding, not yet Jellyfin compatible.** The server boots a provisional API, health endpoint and admin UI. Read-only scanning, FFprobe inspection and a private SQLite catalog are available via CLI, but the catalog is not exposed as a Jellyfin library yet, and authentication, transcoding, user-account migration and Smart TV playback are not implemented. Full compatibility is an acceptance goal, not a current claim.

## 🎯 MVP means the COMPLETE Jellyfin Server port

- Replace all built-in Jellyfin Server v12.3 C#/.NET application logic with Rust, independently implemented with protocol and behavioral parity. **No .NET runtime or original Jellyfin daemon.**
- Connect **existing, unchanged** Jellyfin Smart TV clients: Android TV, Fire TV, LG webOS, Samsung Tizen, and Roku, plus web and mobile clients.
- Match core library, metadata, search, user/auth, playback, transcode, subtitles, SyncPlay, Live TV, admin, tasks, backup, settings and important first-party features.
- Retain `ffmpeg`, `ffprobe`, Linux filesystems and GPU drivers instead of pointlessly rewriting proven media tools.
- Safely import compatible existing library/user data without changing the user's original Jellyfin installation.

**Full-port acceptance:** all [M0–M6 milestones](docs/ROADMAP.md) must be met. Passing first TV playback is a major gate but **not MVP completion**.

## 📊 Development status

`█░░░░░░░░░ 8%` across 84 atomic tasks (accepted-check arithmetic). This is project completion accounting, **not a claim of passing CI or Smart TV operation**. Every task has evidence-gated checks. See [machine-readable ledger](docs/progress.json), [full roadmap](docs/ROADMAP.md), [source baseline](docs/SOURCE_BASELINE.md), [port matrix](docs/PORT_MATRIX.md) and [TV compatibility contract](docs/COMPATIBILITY.md).

**Latest passing CI:** [run 39954730665](https://github.com/sguzman/pelagos/actions/runs/39954730665) — formatting, Clippy, Rust/UDP/SQLite tests, roadmap and pinned inventory validations.

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
# Opt-in LAN discovery (does NOT enable login or TV streaming):
cargo run -- --bind 0.0.0.0 --port 8097 --discover
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Open `http://127.0.0.1:8097/web/`. JSON diagnostics: `/health`, `/api/pelagos/status`. The unauthenticated prototype deliberately defaults to **loopback-only** and port 8097, avoiding the usual Jellyfin port 8096. UDP discovery is opt-in, on port 7359, only responds to LAN/loopback peers and cannot run alongside another service already bound to that UDP port. Do not expose this build to untrusted networks. Requires a normal Rust toolchain; no Python tooling is required.

## 🦀 Architecture and independence

Rust owns HTTP, protocols, indexing, metadata, state, playback negotiations, and FFmpeg supervision. A lightweight browser admin shell is included as a scaffold. The system supports Linux-first development, with EndeavourOS/Hyprland a primary target. The existing user-installed Jellyfin server remains independent and untouched.

[Architecture](docs/ARCHITECTURE.md) · [Pinned API attributes](docs/API_CONTRACTS.md) · [Compatibility](docs/COMPATIBILITY.md) · [Provenance and licensing](docs/PROVENANCE.md) · [Tracking](docs/ISSUES.md).

Pelagos is independent of and not endorsed by Jellyfin. This repository retains its original MIT license for independently authored implementation; Jellyfin's GPL-2.0 source must not be copied/translated without meeting derivative-work licensing obligations.
