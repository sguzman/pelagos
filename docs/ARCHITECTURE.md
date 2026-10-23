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

## Playback information and catalog v2 migration

The Jellyfin-shaped `GET/POST /Items/{id}/PlaybackInfo` endpoints return conservative media-source records derived by the existing bounded external FFprobe subprocess. The Rust application state supports a locally selected FFprobe executable for deterministic integration fixtures (not supplied by HTTP users). Fields include container, duration ticks, video/audio codec and stream attributes; transcoding and direct remux are deliberately marked unavailable. Unknown playback negotiation is rejected rather than silently simulated. Synthetic authenticated HTTP success and denial tests pass at [CI 39960435059](https://github.com/sguzman/pelagos/actions/runs/39960435059). Unmodified TV acceptance remains untested.

Schema v2 registers every indexed library separately from its media rows, so empty libraries remain visible and browsable. The v1→v2 migration copies existing roots into the new registry inside a SQLite transaction, without changing user media or Jellyfin's database. Tests cover migration, empty→nonempty→empty rescans and REST browsing at [CI 39960935517](https://github.com/sguzman/pelagos/actions/runs/39960935517). A full user-configured library manager remains pending.

## Device-aware authentication sessions

Each successful Jellyfin-shaped password login creates a persistent session record linked by foreign key to the SHA-256 digest of its random token. `Authorization: MediaBrowser/Emby` client, device, device ID and version metadata are parsed with quoted-comma awareness and stored in Pelagos' independent authentication database. Login returns a typed `SessionInfo` with an ID and conservative control flags (no false media/remote-control claims). `GET /Sessions` is authentication-gated, scoped to the owner's sessions unless an administrator is querying, and supports `deviceId`. Until runtime activity tracking exists, `activeWithinSeconds` and `controllableByUserId` explicitly return 501. Logout deletes a token and cascades its session row. [CI 39962550684](https://github.com/sguzman/pelagos/actions/runs/39962550684) passed synthetic user-scope, client-header, token revocation and integration tests. No TV device or reconnect flow has been validated.

## Explicit library permissions and descriptor-confined serving

Pelagos now stores per-user canonical-root read grants in its own authentication SQLite database. New non-admin accounts default to **no** libraries; local CLI `library-grant USER ROOT` and `library-revoke USER ROOT` update authorizations without restarting Pelagos. Library views, item queries, direct static streams and FFprobe playback info resolve the same authenticated scope; unrelated media IDs return 404 rather than leaking existence. Administrator accounts continue to see all indexed libraries. This is library-root authorization only, not complete Jellyfin per-user or parental policy parity. Cross-library and dynamic-revocation tests pass [CI 39965370233](https://github.com/sguzman/pelagos/actions/runs/39965370233).

On Linux, direct static streaming opens a file and verifies `/proc/self/fd/<fd>` against the canonical registered library root **before any bytes are served**; the verified open file handle is reused for byte-range reads, so replacing a pathname later cannot redirect the ongoing response. Replaced root symlinks and outside-root file targets are denied. On unsupported non-Linux systems this specific direct-file handler refuses to serve rather than silently falling back to vulnerable name-based validation. Synthetic swap tests pass [CI 39965980868](https://github.com/sguzman/pelagos/actions/runs/39965980868). Do not treat this targeted safeguard as a full filesystem sandbox or authorization/security audit.

## Search hints and direct audio

The first pinned Jellyfin v12.3 `GET /Search/Hints` implementation reuses exactly the same permission-filtered item graph as `/Items`. It exposes partial SearchHint DTOs for filename-based media search, case-insensitive matching, bounded pagination and recognized item/media type filters. Unknown item types and unsupported movie/series classifications explicitly return 501 rather than inventing metadata. This is not yet artist/people/genre/full-text provider search; [CI 39967003914](https://github.com/sguzman/pelagos/actions/runs/39967003914) uses synthetic catalog and permission fixtures.

`GET/HEAD /Audio/{itemId}/stream?static=true` shares the same authenticated, Linux descriptor-confined, MIME-aware byte-range handler as static video. Explicit non-static/transcoding requests are rejected with 501 until real FFmpeg playback pipelines exist. The audio fixture tests GET, HEAD, authentication and partial content at [CI 39967422588](https://github.com/sguzman/pelagos/actions/runs/39967422588). Codec negotiation, remux, audio transcoding and actual Jellyfin app playback remain unimplemented.

## Shared browsing/search filters

`/Items` and `/Search/Hints` now use common recognized Jellyfin item/media type filter parsers, including comma-delimited `includeItemTypes`, `excludeItemTypes`, and `mediaTypes`, before calculating result totals or applying pagination. The shared parser refuses unknown requested kinds instead of misclassifying a movie/series from its filename. `GET /Items?isMovie=...` and `isSeries` return 501 until real metadata classifiers exist. Synthetic audio/video/pagination tests passed [CI 39968255232](https://github.com/sguzman/pelagos/actions/runs/39968255232). This is only a supported subset of Jellyfin's exhaustive item-query schema, not general API parity.

## Per-user watched/favorite and direct-play resume state

`pelagos-user-data.sqlite3` is an independent Pelagos state store (never Jellyfin's database) keyed by authenticated user and stable catalog item ID, with played, play count, last played UTC, favorite and nonnegative playback position ticks. Jellyfin v12.3-shaped `POST/DELETE /UserPlayedItems/{id}` and `/UserFavoriteItems/{id}` plus deprecated `/Users/{userId}/...` variants are supported as a **subset**, enforcing each account's library access and blocking cross-user mutation. The item browsing API now exposes `UserData` scoped to the bearer and reads paginated records with one SQLite connection; legacy `GET /Users/{userId}/Items/{itemId}` enforces the user's identity. Synthetic authenticated tests pass [CI 39982107696](https://github.com/sguzman/pelagos/actions/runs/39982107696).

For direct-play-only sessions, `POST /Sessions/Playing`, `/Sessions/Playing/Progress`, and `/Sessions/Playing/Stopped` save authorized, nonnegative `PositionTicks`, preserving personal watched/favorite flags, while unsupported remux/transcode methods and incompatible source IDs are explicitly rejected. The API responds 204 for accepted reports. [CI 39982784834](https://github.com/sguzman/pelagos/actions/runs/39982784834) verifies isolation, persistence across server-state reloads, invalid positions and revocation. This is **not** full Jellyfin session management: WebSocket presence, play session lifecycle, heartbeats, playback completion thresholds, and client acceptance remain open.

## Container-suffixed direct stream routes and honest negotiation

Pelagos now recognizes Jellyfin v12.3 `/Videos/{id}/stream.{container}` and `/Audio/{id}/stream.{container}` variants (GET/HEAD), routed through the exact same scoped and descriptor-confined static-file handler. A requested suffix must match the original indexed extension; the Rust server **never** silently converts one container into another. This is validated by synthetic audio/video range tests ([CI 39983627300](https://github.com/sguzman/pelagos/actions/runs/39983627300)).

Likewise, `startTimeTicks` seeking beyond 0, requested `audioCodec`/`videoCodec`, subtitle stream selection, conflicting `mediaSourceId`, or incompatible `container` queries return explicit 501 rather than apparently succeeding while serving unmodified bytes. Negative ticks return 400. [CI 39984094410](https://github.com/sguzman/pelagos/actions/runs/39984094410) validates status contracts. Full remux/transcoding remains a required but unimplemented FFmpeg phase.

## Read-only local posters and collection art

Pelagos now selects **indexed** sidecar images without filesystem exploration outside registered roots. It prefers title-specific `<stem>-poster`/`<stem>` art for individual audio/video items, then local `poster`, `folder` and `cover` files; collection/nested folders select exact-directory sidecars. `ImageTags.Primary` derive from the image root, relative path, current file length and nanosecond modification timestamp when available; no original filesystem paths are exposed. `GET/HEAD /Items/{id}/Images/Primary` and `/Primary/0` serve at most 16 MiB through the same Linux verified opened-file descriptor mechanism as direct streaming. The media-item and folder contracts passed synthetic CI [39987262612](https://github.com/sguzman/pelagos/actions/runs/39987262612) and [39988010040](https://github.com/sguzman/pelagos/actions/runs/39988010040), including grant/revoke and symlink guards.

The initial authenticated `GET /Items/{id}/Images` lists a `Primary` ImageInfo with measured size, tag and null single-image index, without inventing blurhash/dimensions or revealing absolute `Path`. Unknown items return 404, known items without local artwork return an empty list. [CI 39988687987](https://github.com/sguzman/pelagos/actions/runs/39988687987) validates the synthetic API. Artwork resizing, multiple image types, metadata providers and unmodified TV rendering remain required.

## Audio media source negotiation

Audio files are now eligible for FFprobe-backed `GET/POST /Items/{id}/PlaybackInfo`, with conservative audio stream fields and `DirectStreamUrl` using `/Audio/{id}/stream?static=true`. Upstream `EnableDirectStream` and `EnableTranscoding` booleans express fallback permissions, so Pelagos no longer rejects a request solely because a client permits those modes; the returned MediaSource **still** reports `SupportsDirectStream=false` and `SupportsTranscoding=false` until implemented. [CI 39986043745](https://github.com/sguzman/pelagos/actions/runs/39986043745) covers scope, audio metadata and honest capabilities. Direct-play TV acceptance remains untested.

## Media sidecar separation and detailed playback information

Indexed covers and posters stay in the **private SQLite catalog** as source image assets but no longer appear as independent `Photo` DTOs or `Search/Hints` entries when a recognized video/audio sidecar relationship exists. Matching uses explicit directory, media stem and library-root constraints; unrelated photographs remain browsable, and standalone photo libraries do not disappear. CI [39989870345](https://github.com/sguzman/pelagos/actions/runs/39989870345) exercises item and search separation.

FFprobe `MediaStreams` now expose a conservative larger Jellyfin-compatible subset: profile, bit depth, aspect, pixel format, channel layout, color range/space/transfer/primaries, subtitle dispositions, title, and finite positive rational frame rates without inferring unknown fields. [CI 39990374327](https://github.com/sguzman/pelagos/actions/runs/39990374327) validates synthetic metadata contracts, not codec capability negotiation.

`/Items/{id}/PlaybackInfo` accepts upstream GET/POST `userId` identity constraints and legacy query-over-POST-body precedence (Jellyfin v12.3), including selected source identity, direct-play options and explicit refusal for unsupported tick seeks, stream selections and auto-open live streams. Requests for another user's id receive 403. Negative ticks receive 400; unsupported modes return 501. [CI 39992775206](https://github.com/sguzman/pelagos/actions/runs/39992775206). Device-profile-backed transcoding capability remains unimplemented.

## Descriptor-backed private artwork caching

Primary artwork responses derive a quoted ETag from the original image's library, relative path, opened-file size and modification timestamp, using the **already confined opened descriptor**. Access checks and Linux handle validation still run for conditional requests. Matching `If-None-Match` returns 304 with an empty body, ETag, and `Cache-Control: private, no-cache`; changed sources return image bytes and an updated ETag. Synthetic regressions passed [CI 39993608914](https://github.com/sguzman/pelagos/actions/runs/39993608914); no real TV caching claim.

## Jellyfin bitrate-test HTTP transport

Pinned Jellyfin v12.3's authenticated `GET /Playback/BitrateTest` now responds with `application/octet-stream` and exactly the requested number of random bytes (default 102400; allowed 1 through 100000000), `Cache-Control: no-store` and matching Content-Length. A Rust Tokio AsyncRead produces at most 16 KiB of fresh randomness per poll; the server never allocates the complete response or invokes FFmpeg for a bandwidth probe. Invalid sizes return 400. [CI 39994640474](https://github.com/sguzman/pelagos/actions/runs/39994640474) validates authentication, sizes, streaming and errors. Real client bandwidth estimation remains untested.

## Authenticated system identity and capability reporting

The pinned Jellyfin `GET /System/Info` endpoint now requires a Pelagos session and embeds the same ID, name, version, product and operating system as the public identity route. Its capability flags explicitly report no live WebSockets, library monitor or self-restart yet, and do not expose program-data, cache, metadata or logs filesystem paths. This is a deliberately incomplete SystemInfo model rather than a fabricated full server implementation. The synthetic authorization/DTO contracts and existing public endpoint assertions pass [CI 39995545838](https://github.com/sguzman/pelagos/actions/runs/39995545838). Real unmodified client handshake remains unverified.
