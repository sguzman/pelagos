# Pelagos 🌊🦀

**The open sea, under your control.** A Rust-native replacement for Jellyfin Server, designed for compatibility with existing Jellyfin clients, including the **unmodified Smart TV Jellyfin apps**.

![Pelagos ocean-turquoise project color](assets/pelagos-color.svg)

**Identity:** `#1DB2C0` · `oklch(70% 0.115 205)` ([color decision](docs/COLOR.md)). **Administration appearance:** ◐ System (default) / ☀ Light / ☾ Dark, with the selector at the top right.

> **Pre-alpha, not yet Jellyfin compatible.** Rust login with persistent device-aware session records, authenticated system-info capability reporting, scoped `/Sessions` queries, item browsing, virtual folders, read-only scanning, FFprobe and SQLite indexing now exist. FFprobe-backed read-only PlaybackInfo, catalog-v2 persisted empty libraries, permission-scoped Search/Hints, static audio/video byte-range streaming, indexed poster/collection artwork and ImageInfo, personal watched/favorite state and direct-play resume positions are synthetic-test verified. Users/Items responses are incomplete, ordinary accounts require explicit local per-library grants, and there is still no client-accepted authentication, Quick Connect, remux/transcoding, user-data migration or Smart TV playback. Library-authorized **static** video streaming with HTTP byte ranges is available for synthetic test media. Full compatibility is a mandatory acceptance goal, not a current claim.

## 🎯 MVP means the COMPLETE Jellyfin Server port

- Replace all built-in Jellyfin Server v12.3 C#/.NET application logic with Rust, independently implemented with protocol and behavioral parity. **No .NET runtime or original Jellyfin daemon.**
- Connect **existing, unchanged** Jellyfin Smart TV clients: Android TV, Fire TV, LG webOS, Samsung Tizen, and Roku, plus web and mobile clients.
- Match core library, metadata, search, user/auth, playback, transcode, subtitles, SyncPlay, Live TV, admin, tasks, backup, settings and important first-party features.
- Retain `ffmpeg`, `ffprobe`, Linux filesystems and GPU drivers instead of pointlessly rewriting proven media tools.
- Safely import compatible existing library/user data without changing the user's original Jellyfin installation.

**Full-port acceptance:** all [M0–M6 milestones](docs/ROADMAP.md) must be met. Passing first TV playback is a major gate but **not MVP completion**.

## 📊 Development status

`███░░░░░░░ 30%` across 135 atomic tasks (accepted-check arithmetic). This is project completion accounting, **not a claim of passing CI or Smart TV operation**. Every task has evidence-gated checks. See [machine-readable ledger](docs/progress.json), [full roadmap](docs/ROADMAP.md), [source baseline](docs/SOURCE_BASELINE.md), [port matrix](docs/PORT_MATRIX.md) and [TV compatibility contract](docs/COMPATIBILITY.md).

**Latest passing CI:** [run 40021221370](https://github.com/sguzman/pelagos/actions/runs/40021221370) — authorized per-series NextUp, resumable filtering, watched progression, full Rust tests, formatting, Clippy and pinned contract guards.

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
# Opt-in LAN discovery; this does not establish TV login/playback compatibility:
cargo run -- --bind 0.0.0.0 --port 8097 --discover
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Open `http://127.0.0.1:8097/web/`. JSON diagnostics: `/health`, `/api/pelagos/status`. The partially authenticated prototype deliberately defaults to **loopback-only** and port 8097, avoiding the usual Jellyfin port 8096. UDP discovery is opt-in, on port 7359, only responds to LAN/loopback peers and cannot run alongside another service already bound to that UDP port. Do not expose this build to untrusted networks. Requires a normal Rust toolchain; no Python tooling is required.

Recognized media sidecar images stay indexed but are hidden from standalone photo browsing and Search/Hints, preserving unrelated photos. Artwork GET/HEAD responses now advertise private ETags and return 304 for matching validators; edits invalidate the tag. Primary local artwork is indexed read-only from poster/cover/folder sidecars: video, audio, library and nested-folder DTOs can report `ImageTags.Primary`; `GET/HEAD /Items/{id}/Images/Primary[/0]` and `GET /Items/{id}/Images` are scoped, synthetic-tested subsets. Artwork resizing, images from external providers, blurhash/dimensions and real app compatibility are still open. New partial user-data routes include `POST/DELETE /UserPlayedItems/{id}`, `POST/DELETE /UserFavoriteItems/{id}`, and `POST /Sessions/Playing[/Progress|/Stopped]`. Per-user watched/favorite and direct-play resume positions persist in a separate Pelagos database; item DTOs expose authenticated `UserData`. An authenticated, chunk-streamed `GET /Playback/BitrateTest` supports Jellyfin's 1..100000000-byte range without allocating a whole response. PlaybackInfo GET/POST now check requested `userId`, apply Jellyfin deprecated query-over-body parameter precedence, and explicitly reject unsupported seek/live/stream-selection controls. Session reporting does **not** yet implement live presence, auto-completion thresholds, WebSocket notifications or non-static playback. The provisional HTTP routes include `GET /Search/Hints`, `GET/HEAD /Audio/{id}/stream?static=true`, and `POST /Users/AuthenticateByName`, `GET /Users/Me`, `POST /Sessions/Logout`, `GET /UserViews`, `GET /Items`, `GET /Items/{id}` and `GET /Users/{user_id}/Items`. The catalog and search routes support recognized type/media filters and require authentication; administrators can see all indexed libraries, while non-administrators have default-deny access with explicit root grants. A user with library access may request `GET /Videos/{id}/stream?static=true` or `HEAD` for a previously indexed, unchanged video file, including a single HTTP `Range` header. Non-static/transcoding requests explicitly return 501. Audio and video static direct play share the same authorization, byte-range and checked-file implementation. Matching original extensions can also be requested through Jellyfin-shaped `/Videos/{id}/stream.mp4` and `/Audio/{id}/stream.mp3` routes; incompatible container, codec, subtitle or tick-seek requests do not silently claim conversion. On Linux the opened file descriptor is checked against its canonical indexed root before media bytes are sent, guarding the tested symlink-swap race. This is a narrow synthetic-tested direct-file stream, not proof an unmodified TV client works.

Jellyfin's pinned `GET /Items/{id}/Ancestors` now returns a private immediate-parent-first folder chain with per-user data. `GET /UserItems/{id}/UserData` and the legacy `/Users/{userId}/Items/{id}/UserData` return locally persisted per-user watched, favorite and direct-play position fields; cross-account requests are denied and revoked-library items remain undiscoverable. These are synthetic contract subsets, not verified unmodified client navigation or synchronization.

The current authenticated `GET /Users/{userId}` only resolves the current account; it does not implement administrator enumeration. Indexed media item DTOs now expose a filename-derived `Container`, but no inferred movie or series classification. The pinned UserData POST route and its legacy URL accept transactional, authorized updates to Played, IsFavorite, PlayCount and PlaybackPositionTicks. Unsupported non-null metadata fields return an explicit error rather than being dropped. These subsets have synthetic CI evidence only; full Jellyfin client behavior is not established.

`GET /UserItems/Resume` and legacy `GET /Users/{userId}/Items/Resume` now return permission-scoped video/audio items with positive saved resume ticks, filters, pagination and personal UserData. Positive direct-play progress records an activity timestamp for sorting; unsupported active-session exclusions and unsupported metadata options are rejected. This is not a full Jellyfin resumable-item engine or a real client-tested resume shelf.

A new authenticated `GET /Items/Root` (and legacy `/Users/{userId}/Items/Root`) provides a stable virtual user root. `GET /Items?parentId=<rootId>` navigates only permission-granted collection folders, including empty libraries; ancestor chains connect them to the virtual root. `/Items` now validates supported `SortName`, ascending/descending order, user ownership and image/UserData suppression; unsupported sort and field projections return 501. These are synthetic-tested subsets, not full upstream query or real-client parity.

The scoped `GET /Library/MediaFolders` exposes indexed collection roots, including empty libraries, to administrators only. Ordinary accounts remain forbidden even with media grants, and physical filesystem paths are not included in these DTOs. Hidden/disabled library configuration and full Jellyfin admin behavior remain unimplemented.

System info now reports `LocalAddress` from a concrete listening IPv4/IPv6 socket, using the actual bound port, never from the incoming HTTP `Host` header. Wildcard binds intentionally return null rather than an invalid `0.0.0.0` or `::` URL. UDP LAN discovery continues to choose its address per requester. This network contract remains synthetic-tested; actual Smart TV connection is not verified.

`GET /QuickConnect/Enabled` now returns `false`, while disabled initiation, secret polling and authorization refuse requests without issuing pairing credentials. This deliberately does **not** implement Jellyfin Quick Connect; manual credentials remain the only preliminary login path. Resume queries now have deterministic automated tests for saved-position newest-first ordering, including when UserData is omitted from display. Both features still require real-client acceptance.

Conservative local SxxEyy recognition now identifies Series, Season and Episode objects only under season-directory agreement, retaining generic Video for unrecognized files. Stable SeriesId/SeasonId references connect the existing virtual folder IDs. Preliminary `GET /Shows/{seriesId}/Seasons` and `/Shows/{seriesId}/Episodes` expose permission-scoped results, numeric season/episode order, specials, paging and `startItemId` continuation. Unsupported options refuse rather than imply metadata that Pelagos lacks. Full naming parity, missing-episode discovery, NextUp and unmodified client acceptance remain open.

Conservative local Movie recognition now requires the video filename and containing folder to match `Title (YYYY)`; otherwise the indexed file stays a generic Video. The Movie DTO exposes a constrained title and `ProductionYear`, and the shared catalog/search filters recognize Movie and the `isMovie` query. Existing stable indexed IDs, playback-position state, and per-library access controls remain in place. This is filename evidence only, not automatic movie classification, NFO parsing, provider metadata or verified client parity.

`GET /Items/Counts` now reports visible indexed media totals and recognized Movie/Series/Episode counts, honoring the authenticated user's own `isFavorite` state. Other categories remain zero until actual matching classification exists; those zeros are **not** evidence of full Jellyfin counts parity. Explicit `isSeries`/`isMovie` filters now work across `/Items` and `/Search/Hints`, including typed Series discovery, without changing default file-only browsing or library access controls.

Personal watched/favorite/resume metadata now loads via bounded SQLite batches rather than a separate query for every media item; regression coverage includes 900+ IDs and enforces isolation and input-order preservation. The pinned `GET /Library/PhysicalPaths` provides administrators only with configured indexed filesystem roots, while ordinary media DTOs remain pathless. Neither change establishes full upstream migration or real client parity.

Pelagos now serves pinned `GET /Items/{itemId}/File` and HEAD using its indexed, authorized, Linux descriptor-confined original-media reader. Byte ranges, video/audio/photo MIME types, and library revocation are synthetic-tested. This **does not** enable the separate `/Items/{itemId}/Download` policy, transcoding, or actual Jellyfin client acceptance.

Pelagos now persists a first-indexed timestamp in catalog schema v3, preserving existing items across rescans rather than misrepresenting every refresh as newly added. Pinned `GET /Items/Latest` and legacy user route provide permission-scoped, newest-first **ungrouped** media when callers explicitly use `groupItems=false`. Upstream defaults to grouped shelves; unsupported/default grouping returns 501 until genuine grouping is implemented. Historical v1/v2 data uses the last-seen timestamp as the best available backfill, not a claimed original creation date.

`GET /Items` and `GET /Search/Hints` now honor `isPlayed` and `isFavorite` from the authenticated account's persisted user data. Filtering occurs before limits and pagination; explicit `enableUserData=false` suppresses returned personal state without disabling correct filtering. No parent or series-wide watched-state inference is claimed. Tests verify that library revocation and account isolation apply equally to these filtered views.

The latest-media API now supports Jellyfin's Episode-to-Series grouping when explicitly restricted to `includeItemTypes=Episode` (with default `groupItems=true` or explicit true). A Series replaces its individual episodes when multiple matching episodes exist, with a truthful `ChildCount`; a singleton remains an Episode. Group counts are computed before applying the requested result limit. Other default/mixed latest grouping still returns 501 until audio albums, photo albums, and full collection-type rules are modeled. This is native HTTP-tested, **not** unmodified-client acceptance.

The pinned `GET /Shows/NextUp` has an explicitly **per-series** subset: callers supply `seriesId`, and Pelagos selects the first recognized unplayed episode after the highest watched season/episode index. `enableResumable=false` can suppress the selected episode if it has saved progress; `startIndex`, `limit`, `enableImages` and `enableUserData` are scoped to the authenticated viewer. Broad NextUp discovery, episode versions, specials, cutoff and rewatching remain unsupported, and unmodified-client compatibility is not claimed.

## 🦀 Architecture and independence

Rust owns HTTP, protocols, indexing, metadata, state, playback negotiations, and FFmpeg supervision. A lightweight browser admin shell is included as a scaffold. The system supports Linux-first development, with EndeavourOS/Hyprland a primary target. The existing user-installed Jellyfin server remains independent and untouched.

[Architecture](docs/ARCHITECTURE.md) · [Pinned API attributes](docs/API_CONTRACTS.md) · [Compatibility](docs/COMPATIBILITY.md) · [Provenance and licensing](docs/PROVENANCE.md) · [Tracking](docs/ISSUES.md).

Pelagos is independent of and not endorsed by Jellyfin. This repository retains its original MIT license for independently authored implementation; Jellyfin's GPL-2.0 source must not be copied/translated without meeting derivative-work licensing obligations.
