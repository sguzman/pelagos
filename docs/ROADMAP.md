# 🗺️ Pelagos full-port roadmap

**MVP means complete Jellyfin Server port**, not a slim movie streamer. Scope: [Jellyfin v12.3 baseline](SOURCE_BASELINE.md) plus compatibility with unmodified Smart TV Jellyfin clients. Every milestone M0 through M6 is part of MVP. Full source of truth: [progress.json](progress.json).

**Overall acceptance progress:** `██░░░░░░░░ 22%` across 111 atomic tasks. This tracks accepted checks, not lines translated or claimed runtime readiness. Checked source-path milestones are not substitutes for passing CI or real devices.

## 🧱 M0 — Foundation and project controls — `███░░░░░░░ 31%`

**Gate:** Native Rust code builds, documentation is traceable, and controls can be tested without running Jellyfin.

### 🧭 P0 — Identity and scope — `█████░░░░░ 47%`

- 🦀 **T001 — Standalone Rust server bootstrap** — `███████░░░ 67%`
  - [x] Implement standalone Rust daemon and HTTP routes — Cargo.toml; src/main.rs; src/app.rs
  - [x] CI compiles server and passes native HTTP route tests — GitHub Actions 39950134726: format, clippy, Rust HTTP/identity tests and ledger-check all succeeded
  - [ ] Production or real-client acceptance validates standalone Rust daemon and HTTP routes
- 🪪 **T002 — Persistent server identity** — `███████░░░ 67%`
  - [x] Implement restart-stable local server ID — src/identity.rs
  - [x] CI validates restart-persistent server identity and corrupt-ID rejection — GitHub Actions 39950134726: format, clippy, Rust HTTP/identity tests and ledger-check all succeeded
  - [ ] Production or real-client acceptance validates restart-stable local server ID
- 🌗 **T003 — Admin web appearance** — `███░░░░░░░ 33%`
  - [x] Implement top-right System/Light/Dark selection with System default — web/index.html
  - [ ] Automated tests and Jellyfin baseline fixture verify top-right System/Light/Dark selection with System default
  - [ ] Production or real-client acceptance validates top-right System/Light/Dark selection with System default
- 🎨 **T004 — Perceptual marine identity** — `███░░░░░░░ 33%`
  - [x] Implement OKLCH color specification and readable SVG swatch — docs/COLOR.md; assets/pelagos-color.svg
  - [ ] Automated tests and Jellyfin baseline fixture verify OKLCH color specification and readable SVG swatch
  - [ ] Production or real-client acceptance validates OKLCH color specification and readable SVG swatch
- 📌 **T005 — Pin Jellyfin reference** — `███░░░░░░░ 33%`
  - [x] Implement baseline source/version and compatibility inventory — docs/SOURCE_BASELINE.md; docs/PORT_MATRIX.md
  - [ ] Automated tests and Jellyfin baseline fixture verify baseline source/version and compatibility inventory
  - [ ] Production or real-client acceptance validates baseline source/version and compatibility inventory

### 🛠️ P1 — Build and storage — `██░░░░░░░░ 22%`

- 🏗️ **T006 — Build reproducibility** — `░░░░░░░░░░ 0%`
  - [ ] Implement locked dependencies and automated stable Rust compilation
  - [ ] Automated tests and Jellyfin baseline fixture verify locked dependencies and automated stable Rust compilation
  - [ ] Production or real-client acceptance validates locked dependencies and automated stable Rust compilation
- 🗄️ **T007 — SQLite store and migrations** — `░░░░░░░░░░ 0%`
  - [ ] Implement versioned schema, atomic transactions and recovery
  - [ ] Automated tests and Jellyfin baseline fixture verify versioned schema, atomic transactions and recovery
  - [ ] Production or real-client acceptance validates versioned schema, atomic transactions and recovery
- ⚙️ **T008 — Validated configuration** — `░░░░░░░░░░ 0%`
  - [ ] Implement XDG config paths, CLI options, migration and safe defaults
  - [ ] Automated tests and Jellyfin baseline fixture verify XDG config paths, CLI options, migration and safe defaults
  - [ ] Production or real-client acceptance validates XDG config paths, CLI options, migration and safe defaults
- 🧯 **T009 — Health, logs and process lifecycle** — `░░░░░░░░░░ 0%`
  - [ ] Implement structured diagnostics, graceful shutdown and signals
  - [ ] Automated tests and Jellyfin baseline fixture verify structured diagnostics, graceful shutdown and signals
  - [ ] Production or real-client acceptance validates structured diagnostics, graceful shutdown and signals
- 📂 **T014 — Read-only local media scan diagnostic** — `███████░░░ 67%`
  - [x] Implement native Rust file-type inventory without following symlinks or modifying media — src/media.rs; src/main.rs; Cargo.toml
  - [x] CI validates synthetic media detection, symlink avoidance and unchanged file contents — GitHub Actions 39950651357: format, clippy, tests and roadmap verification passed
  - [ ] Live Linux scan verifies a real user-authorized library safely
- 🗄️ **T016 — Isolated transactional SQLite media catalog** — `███████░░░ 67%`
  - [x] Create a separate Pelagos database and atomic read-only-source library index — src/catalog.rs; Cargo.toml; src/main.rs — versioned SQLite schema and index/catalog CLI
  - [x] CI verifies incrementals, stale records, library isolation, symlinks and schema safety — GitHub Actions 39954730665 — Rust catalog tests, clippy, format and tracking checks pass
  - [ ] Large real-media library and recovery/upgrade QA passes under Linux

### 🧪 P2 — Test framework — `███░░░░░░░ 27%`

- 📊 **T010 — Ledger generator and CI guard** — `███████░░░ 67%`
  - [x] Implement machine ledger, calculated percentages and acceptance evidence — src/bin/pelagos-roadmap.rs; docs/progress.json; docs/ROADMAP.md
  - [x] Automated CI verifies ledger evidence, task percentages and generated roadmap are in sync — GitHub Actions 39950134726: format, clippy, Rust HTTP/identity tests and ledger-check all succeeded
  - [ ] Review and prove milestone gate and acceptance evidence completeness
- 🧰 **T011 — Protocol fixtures and mock clients** — `░░░░░░░░░░ 0%`
  - [ ] Implement contract replay harness without private user data
  - [ ] Automated tests and Jellyfin baseline fixture verify contract replay harness without private user data
  - [ ] Production or real-client acceptance validates contract replay harness without private user data
- 🔒 **T012 — Security harness** — `░░░░░░░░░░ 0%`
  - [ ] Implement authentication, authorization, path traversal and fuzz regression suite
  - [ ] Automated tests and Jellyfin baseline fixture verify authentication, authorization, path traversal and fuzz regression suite
  - [ ] Production or real-client acceptance validates authentication, authorization, path traversal and fuzz regression suite
- 🖥️ **T013 — Rendered admin GUI acceptance** — `░░░░░░░░░░ 0%`
  - [ ] Implement keyboard, responsive, contrast and three-mode browser screenshots
  - [ ] Automated tests and Jellyfin baseline fixture verify keyboard, responsive, contrast and three-mode browser screenshots
  - [ ] Production or real-client acceptance validates keyboard, responsive, contrast and three-mode browser screenshots
- 📡 **T015 — Pinned HTTP controller attribute inventory** — `███████░░░ 67%`
  - [x] Inventory every Jellyfin v12.3 root API controller and HTTP attribute with source SHA — docs/api-contracts.json; docs/API_CONTRACTS.md — 60 pinned controllers, 419 method attributes
  - [x] Rust CI rejects missing, malformed or silently changed pinned controller inventory — GitHub Actions 39953180557 — pelagos-api-matrix test and check pass
  - [ ] Differential OpenAPI and real-client verification resolves route and DTO coverage

## 🔌 M1 — Jellyfin protocol and API parity — `███░░░░░░░ 30%`

**Gate:** Unmodified Jellyfin clients can discover, authenticate, browse and maintain sessions without C#.

### 🌐 P3 — System and transports — `██░░░░░░░░ 22%`

- 📡 **T100 — Discovery and public server info** — `░░░░░░░░░░ 0%`
  - [ ] Implement Jellyfin discovery, public system info and stable version negotiation
  - [ ] Automated tests and Jellyfin baseline fixture verify Jellyfin discovery, public system info and stable version negotiation
  - [ ] Production or real-client acceptance validates Jellyfin discovery, public system info and stable version negotiation
- 📨 **T101 — API transport contracts** — `░░░░░░░░░░ 0%`
  - [ ] Implement headers, error models, casing, pagination and compatibility responses
  - [ ] Automated tests and Jellyfin baseline fixture verify headers, error models, casing, pagination and compatibility responses
  - [ ] Production or real-client acceptance validates headers, error models, casing, pagination and compatibility responses
- 🔁 **T102 — WebSocket sessions** — `░░░░░░░░░░ 0%`
  - [ ] Implement Jellyfin WebSocket events, heartbeats and reconnects
  - [ ] Automated tests and Jellyfin baseline fixture verify Jellyfin WebSocket events, heartbeats and reconnects
  - [ ] Production or real-client acceptance validates Jellyfin WebSocket events, heartbeats and reconnects
- 🛰️ **T103 — Device pairing and Quick Connect** — `░░░░░░░░░░ 0%`
  - [ ] Implement TV-friendly Quick Connect and device approval
  - [ ] Automated tests and Jellyfin baseline fixture verify TV-friendly Quick Connect and device approval
  - [ ] Production or real-client acceptance validates TV-friendly Quick Connect and device approval
- 📺 **T104 — Jellyfin-compatible opt-in UDP LAN discovery** — `███████░░░ 67%`
  - [x] Implement Jellyfin v12.3 UDP 7359 request/response with accurate LAN URL — src/discovery.rs; src/main.rs — opt-in discovery, local-peer checks, stable ID
  - [x] Automated loopback UDP, message filtering, JSON property and address tests pass — GitHub Actions 39954060555 — formatting, Clippy, UDP tests and other checks pass
  - [ ] Unmodified Smart TV Jellyfin app actually discovers Pelagos and verifies address
- 🖥️ **T105 — Authenticated system information with explicit capability flags** — `███████░░░ 67%`
  - [x] Expose authenticated Jellyfin GET /System/Info consistent with public server ID; omit private filesystem paths and unimplemented runtime claims — src/app.rs — flattened PublicSystemInfo and conservative SystemInfo capabilities with authenticated boundary
  - [x] CI tests anonymous rejection, public/private identity agreement and honest system flags — GitHub Actions 39995545838 — full Rust system info and pinned controller regression tests passed
  - [ ] Unmodified Jellyfin Smart TV and administration clients accept the system-info responses

### 👥 P4 — Authentication and user state — `███░░░░░░░ 34%`

- 🔑 **T110 — Users and token issuance** — `░░░░░░░░░░ 0%`
  - [ ] Implement password hashing, access tokens, logout and expiry
  - [ ] Automated tests and Jellyfin baseline fixture verify password hashing, access tokens, logout and expiry
  - [ ] Production or real-client acceptance validates password hashing, access tokens, logout and expiry
- 🛡️ **T111 — Authorization and parental policy** — `░░░░░░░░░░ 0%`
  - [ ] Implement per-library permissions, parental ratings and account restrictions
  - [ ] Automated tests and Jellyfin baseline fixture verify per-library permissions, parental ratings and account restrictions
  - [ ] Production or real-client acceptance validates per-library permissions, parental ratings and account restrictions
- 📱 **T112 — Device authorization** — `░░░░░░░░░░ 0%`
  - [ ] Implement sessions, remote control and device inventory
  - [ ] Automated tests and Jellyfin baseline fixture verify sessions, remote control and device inventory
  - [ ] Production or real-client acceptance validates sessions, remote control and device inventory
- 🕰️ **T113 — Playback state** — `░░░░░░░░░░ 0%`
  - [ ] Implement watch states, resume positions, history and user configuration
  - [ ] Automated tests and Jellyfin baseline fixture verify watch states, resume positions, history and user configuration
  - [ ] Production or real-client acceptance validates watch states, resume positions, history and user configuration
- 🔐 **T114 — Argon2id local credentials and persistent API tokens** — `███████░░░ 67%`
  - [x] Rust creates private accounts, hashes passwords with Argon2id and stores only token digests — src/auth.rs; src/main.rs — first local user admin, 256-bit random tokens and revocation
  - [x] CI verifies login, logout, invalid/disabled accounts, token storage and unauthorized HTTP access — GitHub Actions 39956568616 — native HTTP/user/token tests, Clippy and format passed
  - [ ] Unmodified Jellyfin clients and security/abuse audit validate full auth behavior
- 📱 **T115 — Persistent device sessions and scoped Jellyfin Sessions API** — `███████░░░ 67%`
  - [x] Issue persistent session IDs and per-client device metadata tied to hashed tokens; expose authenticated GET /Sessions with owner/admin filtering — src/auth.rs; src/auth_api.rs — quotes-aware Jellyfin header metadata, FK-backed session rows, revocation and device scoping
  - [x] CI proves device-aware login response, session authorization, unsupported-filter rejection and cascading logout — GitHub Actions 39962550684 — full formatting, Clippy, Rust unit/HTTP tests and pinned matrix checks passed
  - [ ] Unmodified TV clients register devices, maintain WebSocket sessions and resume real playback across reconnects
- 🔒 **T116 — Default-deny per-library permission enforcement** — `███████░░░ 67%`
  - [x] Persist explicit grants for ordinary users and enforce scoped catalog, media and direct streaming access — src/auth.rs; src/auth_api.rs; src/catalog_api.rs; src/streaming.rs; src/playback.rs — SQLite grant mapping and CLI grant/revoke
  - [x] CI proves cross-library isolation, no file-ID disclosure, dynamic grant/revoke and administrator access — GitHub Actions 39965370233 — format, Clippy, Rust permission and HTTP integration tests passed
  - [ ] Real Jellyfin users and unmodified TV apps validate library permission and parental policy parity
- ⭐ **T117 — Persistent watched and favorite user-data writes** — `███████░░░ 67%`
  - [x] Implement per-user played/unplayed and favorite/unfavorite Jellyfin routes with private persistent SQLite state — src/user_data.rs — independent Pelagos database, user-scoped flag updates, play-count and last-played fields
  - [x] CI verifies individual user isolation, restart persistence, repeat handling, legacy endpoint authorization and revocation — GitHub Actions 39981559797 — formatting, Clippy, SQLite and HTTP tests passed
  - [ ] Unmodified Jellyfin TV/mobile clients agree on played state, sync updates and watched-count semantics

### 🧭 P5 — General API contracts — `███░░░░░░░ 34%`

- 🔎 **T120 — Search and query APIs** — `░░░░░░░░░░ 0%`
  - [ ] Implement search, suggestions, sorting, filters and item pagination
  - [ ] Automated tests and Jellyfin baseline fixture verify search, suggestions, sorting, filters and item pagination
  - [ ] Production or real-client acceptance validates search, suggestions, sorting, filters and item pagination
- 🖼️ **T121 — Artwork endpoints** — `░░░░░░░░░░ 0%`
  - [ ] Implement images, sizing, remote art and caches
  - [ ] Automated tests and Jellyfin baseline fixture verify images, sizing, remote art and caches
  - [ ] Production or real-client acceptance validates images, sizing, remote art and caches
- ⚡ **T122 — Activity and tasks APIs** — `░░░░░░░░░░ 0%`
  - [ ] Implement scheduled job state, activity log and configuration endpoints
  - [ ] Automated tests and Jellyfin baseline fixture verify scheduled job state, activity log and configuration endpoints
  - [ ] Production or real-client acceptance validates scheduled job state, activity log and configuration endpoints
- 🧪 **T123 — Actual TV login smoke suite** — `░░░░░░░░░░ 0%`
  - [ ] Implement unmodified Android TV, webOS/Tizen and Roku client login fixtures
  - [ ] Automated tests and Jellyfin baseline fixture verify unmodified Android TV, webOS/Tizen and Roku client login fixtures
  - [ ] Production or real-client acceptance validates unmodified Android TV, webOS/Tizen and Roku client login fixtures
- 🗂️ **T124 — Authenticated preliminary Jellyfin item routes** — `███████░░░ 67%`
  - [x] Implement administrator-only Items/ID and user views from Pelagos SQLite — src/catalog_api.rs; src/app.rs — items, paging, search and library views; absolute paths withheld
  - [x] CI passes synthetic HTTP catalog pagination and account isolation tests — GitHub Actions 39957337582 — library HTTP and credential integration tests passed
  - [ ] Real Jellyfin clients validate full BaseItemDto, authorization and item query coverage
- 🔎 **T125 — Authorized initial Jellyfin Search/Hints API** — `███████░░░ 67%`
  - [x] Serve Jellyfin v12.3 SearchHints DTO from indexed file names with filters, pagination and user scoping — src/search.rs; src/catalog_api.rs — shared accessible catalog graph and bounded media-name hints
  - [x] CI proves case-insensitive search, exclude and type queries, pagination and no cross-library disclosure — GitHub Actions 39968688051 — Rust Search/Hints tests for case-insensitive names, include/exclude and media filters, parent root scoping, pagination, permissions, Clippy and format
  - [ ] Unmodified Jellyfin clients exercise full title/artist/genre/people and metadata-aware search
- 🧰 **T126 — Consistent recognized Items and Search media filters** — `███████░░░ 67%`
  - [x] Apply recognized include/exclude item kinds and media types before pagination across Items endpoints — src/query_filters.rs; src/catalog_api.rs; src/search.rs — shared strict type filters and unsupported Movie/Series refusal
  - [x] CI verifies audio/video item filters, pagination, invalid kinds and search-contract consistency — GitHub Actions 39968255232 — formatting, Clippy, filter HTTP tests and source ledger validations passed
  - [ ] Real Jellyfin client pagination, library filtering and full upstream query semantics verified
- 🪪 **T127 — Authenticated UserData in item DTOs and legacy lookup** — `███████░░░ 67%`
  - [x] Include per-account UserData in /Items and item detail with paginated SQLite batching and legacy /Users/{userId}/Items/{id} — src/catalog_api.rs; src/user_data.rs — one read connection per paginated response, permission gating and legacy ownership check
  - [x] CI verifies per-user playback/favorite DTO fields and access isolation — GitHub Actions 39982107696 — Rust item/legacy user data integration tests and source checks passed
  - [ ] Real Jellyfin clients validate complete BaseItemDto UserData schema and updates across browsing views

## 📚 M2 — Full media library and metadata — `███░░░░░░░ 25%`

**Gate:** Every core library type can be scanned, indexed, enriched, queried and updated like Jellyfin.

### 📂 P6 — Filesystem and items — `█░░░░░░░░░ 13%`

- 🗂️ **T200 — Library roots and filesystem watching** — `░░░░░░░░░░ 0%`
  - [ ] Implement multi-root libraries, watcher updates and rescan correctness
  - [ ] Automated tests and Jellyfin baseline fixture verify multi-root libraries, watcher updates and rescan correctness
  - [ ] Production or real-client acceptance validates multi-root libraries, watcher updates and rescan correctness
- 📀 **T201 — Media naming and structure** — `░░░░░░░░░░ 0%`
  - [ ] Implement series, movies, editions, seasons, episodes and extras naming parity
  - [ ] Automated tests and Jellyfin baseline fixture verify series, movies, editions, seasons, episodes and extras naming parity
  - [ ] Production or real-client acceptance validates series, movies, editions, seasons, episodes and extras naming parity
- 🧬 **T202 — Item graph and persistence** — `░░░░░░░░░░ 0%`
  - [ ] Implement item hierarchy, people, genres, studios and provider identities
  - [ ] Automated tests and Jellyfin baseline fixture verify item hierarchy, people, genres, studios and provider identities
  - [ ] Production or real-client acceptance validates item hierarchy, people, genres, studios and provider identities
- 🪄 **T203 — Library jobs and cleanup** — `░░░░░░░░░░ 0%`
  - [ ] Implement refreshes, scheduled tasks, deletion safety and retention
  - [ ] Automated tests and Jellyfin baseline fixture verify refreshes, scheduled tasks, deletion safety and retention
  - [ ] Production or real-client acceptance validates refreshes, scheduled tasks, deletion safety and retention
- 🗃️ **T204 — Persistent empty libraries and v1→v2 catalog migration** — `███████░░░ 67%`
  - [x] V2 SQLite schema registers empty media libraries independently from file rows and migrates old roots — src/catalog.rs — transaction-preserving catalog migration, explicit library roots, stale-row cleanup
  - [x] CI validates v1 migration, empty-library rescans, /UserViews and empty parent navigation — GitHub Actions 39960935517 — format, Clippy and Rust synthetic migration and REST tests passed
  - [ ] Live Jellyfin clients browse renamed, deleted and empty production libraries across upgrades

### 🧩 P7 — Metadata systems — `███░░░░░░░ 34%`

- 📝 **T210 — Metadata readers** — `░░░░░░░░░░ 0%`
  - [ ] Implement NFO, tags, EXIF, chapters and sidecar parsing
  - [ ] Automated tests and Jellyfin baseline fixture verify NFO, tags, EXIF, chapters and sidecar parsing
  - [ ] Production or real-client acceptance validates NFO, tags, EXIF, chapters and sidecar parsing
- 🛜 **T211 — Metadata providers** — `░░░░░░░░░░ 0%`
  - [ ] Implement provider fetching, merging, rate limits and fallback
  - [ ] Automated tests and Jellyfin baseline fixture verify provider fetching, merging, rate limits and fallback
  - [ ] Production or real-client acceptance validates provider fetching, merging, rate limits and fallback
- 🖌️ **T212 — Images and thumbnails** — `░░░░░░░░░░ 0%`
  - [ ] Implement posters, backdrops, chapter thumbnails and caching
  - [ ] Automated tests and Jellyfin baseline fixture verify posters, backdrops, chapter thumbnails and caching
  - [ ] Production or real-client acceptance validates posters, backdrops, chapter thumbnails and caching
- 🌎 **T213 — Locale, subtitles and titles** — `░░░░░░░░░░ 0%`
  - [ ] Implement language, localization, provider IDs and sort-title rules
  - [ ] Automated tests and Jellyfin baseline fixture verify language, localization, provider IDs and sort-title rules
  - [ ] Production or real-client acceptance validates language, localization, provider IDs and sort-title rules
- 🖼️ **T214 — Indexed primary posters and cover artwork for individual media** — `███████░░░ 67%`
  - [x] Find local indexed title posters or cover files, advertise ImageTags.Primary and serve scope-checked Primary GET/HEAD — src/artwork.rs; src/catalog_api.rs — deterministic local image sidecar selection and Linux descriptor validation
  - [x] CI verifies authenticated HTTP contract, representative fixtures and Rust quality gates — GitHub Actions 39987262612 — title-specific precedence, image response/MIME, owner scope, revocation, symlink guard tests passed
  - [ ] Unmodified Jellyfin Smart TV clients validate this behavior with real libraries and metadata
- 📁 **T215 — Primary artwork for library and nested collection folders** — `███████░░░ 67%`
  - [x] Resolve indexed poster/folder/cover art for collection roots and nested album folders through Primary and Primary/0 URLs — src/artwork.rs; src/catalog_api.rs — virtual folder image tags, guarded nested-folder resolution and empty-library views
  - [x] CI verifies authenticated HTTP contract, representative fixtures and Rust quality gates — GitHub Actions 39988010040 — album/root media artwork, indexed /0, user grants/revocation, formatter and Clippy passed
  - [ ] Unmodified Jellyfin Smart TV clients validate this behavior with real libraries and metadata
- 🧩 **T216 — Separate indexed media sidecars from independent photo items** — `███████░░░ 67%`
  - [x] Classify title/cover/poster/folder sidecars using media-root and directory matching; leave independent photo items visible and originals indexed — src/artwork.rs sidecar_keys; src/catalog_api.rs catalog_nodes — scoped separation of display items and indexed image assets
  - [x] CI verifies photo browse/search semantics, album artwork retention and unrelated photo libraries — GitHub Actions 39989870345 — Rust artwork, catalog HTTP, search and security regression suites passed
  - [ ] Unmodified TV and photo clients validate cover classification across diverse real library layouts
- 🏷️ **T217 — Private conditional ETag validation for indexed artwork** — `███████░░░ 67%`
  - [x] Return descriptor-backed private ETag validators; match If-None-Match with empty 304 — src/artwork.rs — source metadata-based validator on verified opened file handle
  - [x] CI proves 304 without bytes and invalidation after edit without reindexing — GitHub Actions 39993608914 — artwork integration tests, permissions, format and Clippy passed
  - [ ] Unmodified Jellyfin TV/image clients validate conditional artwork fetches and account privacy

### 🎼 P8 — Library feature APIs — `██░░░░░░░░ 22%`

- 🎬 **T220 — Movie and TV collections** — `░░░░░░░░░░ 0%`
  - [ ] Implement movies, series, playlists, collections and multiple versions
  - [ ] Automated tests and Jellyfin baseline fixture verify movies, series, playlists, collections and multiple versions
  - [ ] Production or real-client acceptance validates movies, series, playlists, collections and multiple versions
- 🎵 **T221 — Music catalog** — `░░░░░░░░░░ 0%`
  - [ ] Implement artists, albums, tracks, genres, lyrics and artist artwork
  - [ ] Automated tests and Jellyfin baseline fixture verify artists, albums, tracks, genres, lyrics and artist artwork
  - [ ] Production or real-client acceptance validates artists, albums, tracks, genres, lyrics and artist artwork
- 📖 **T222 — Books and photo libraries** — `░░░░░░░░░░ 0%`
  - [ ] Implement books, images, home videos and mixed content
  - [ ] Automated tests and Jellyfin baseline fixture verify books, images, home videos and mixed content
  - [ ] Production or real-client acceptance validates books, images, home videos and mixed content
- 🔗 **T223 — User media edits** — `░░░░░░░░░░ 0%`
  - [ ] Implement favorite, rating, playlist editing and metadata override round trips
  - [ ] Automated tests and Jellyfin baseline fixture verify favorite, rating, playlist editing and metadata override round trips
  - [ ] Production or real-client acceptance validates favorite, rating, playlist editing and metadata override round trips
- 🧭 **T225 — Virtual folder tree and recursive item navigation** — `███████░░░ 67%`
  - [x] Implement stable collection/folder IDs and ParentId hierarchy from indexed filenames — src/catalog_api.rs — folder nodes, direct child and recursive file queries
  - [x] CI verifies nested synthetic library folder navigation, item-ID and missing-parent behavior — GitHub Actions 39957906828 — Rust/HTTP tests, Clippy, format and ledger checks succeeded
  - [ ] Unmodified Jellyfin TV clients browse seasons, metadata and artwork in real libraries
- 🏷️ **T226 — Permission-scoped primary ImageInfo collection API** — `███████░░░ 67%`
  - [x] List locally indexed primary image tags and actual sizes via GET /Items/{id}/Images without raw paths or fabricated dimensions — src/artwork.rs — authenticated Jellyfin ImageInfo subset, read-only metadata and missing-image behavior
  - [x] CI verifies authenticated HTTP contract, representative fixtures and Rust quality gates — GitHub Actions 39988687987 — scoped ImageInfo listing, privacy, HTTP and Rust validation passed
  - [ ] Unmodified Jellyfin Smart TV clients validate this behavior with real libraries and metadata

## 🎞️ M3 — Playback, FFmpeg and hardware acceleration — `███░░░░░░░ 34%`

**Gate:** Existing Jellyfin TV apps direct-play, remux, transcode and seek real media correctly.

### ▶️ P9 — Streaming engine — `█████░░░░░ 50%`

- 📄 **T300 — Media probing** — `░░░░░░░░░░ 0%`
  - [ ] Implement ffprobe output mapping and device profiles
  - [ ] Automated tests and Jellyfin baseline fixture verify ffprobe output mapping and device profiles
  - [ ] Production or real-client acceptance validates ffprobe output mapping and device profiles
- 🎯 **T301 — Playback negotiation** — `░░░░░░░░░░ 0%`
  - [ ] Implement client profiles, direct play, direct stream and transcoding decisions
  - [ ] Automated tests and Jellyfin baseline fixture verify client profiles, direct play, direct stream and transcoding decisions
  - [ ] Production or real-client acceptance validates client profiles, direct play, direct stream and transcoding decisions
- 🚰 **T302 — Direct streaming** — `░░░░░░░░░░ 0%`
  - [ ] Implement range requests, MIME behavior, seeking and large-file streaming
  - [ ] Automated tests and Jellyfin baseline fixture verify range requests, MIME behavior, seeking and large-file streaming
  - [ ] Production or real-client acceptance validates range requests, MIME behavior, seeking and large-file streaming
- 🔁 **T303 — Remux and media sources** — `░░░░░░░░░░ 0%`
  - [ ] Implement container remux, alternate versions, segment boundaries and timestamps
  - [ ] Automated tests and Jellyfin baseline fixture verify container remux, alternate versions, segment boundaries and timestamps
  - [ ] Production or real-client acceptance validates container remux, alternate versions, segment boundaries and timestamps
- 🔬 **T304 — Bounded FFprobe process integration** — `███████░░░ 67%`
  - [x] Rust invokes FFprobe read-only with bounded output, timeout and no shell — src/probe.rs; src/main.rs — probe CLI and isolated subprocess bridge
  - [x] CI passes synthetic FFprobe success, failure, malformed output and deadline regression tests — GitHub Actions 39952135417 — FFprobe process unit tests, Clippy and formatting passed
  - [ ] Real FFprobe executable and real media stream/metadata profiles pass platform acceptance
- 🎬 **T305 — Authenticated static video HTTP byte-range streaming** — `███████░░░ 67%`
  - [x] Implement admin-gated GET/HEAD static video route, MIME, byte ranges and source-root confinement — src/streaming.rs; src/app.rs — direct file streaming via tokio-util; no remux/transcode claim
  - [x] CI tests full/partial/HEAD, 416 ranges, authorization, indexed symlink escape and CLI/API consistency — GitHub Actions 39958988196: formatting, Clippy, Rust HTTP range and symlink regression tests, roadmap/matrix checks passed
  - [ ] Real Jellyfin TV apps direct-play, seek and resume across tested codecs and devices
- 🎛️ **T306 — Conservative FFprobe media-source and playback information API** — `███████░░░ 67%`
  - [x] Rust GET/POST playback information maps FFprobe streams and duration, gates auth and refuses unsupported transcoding requests — src/playback.rs; src/app.rs — bounded probe, direct-source DTO with no absolute host paths
  - [x] CI validates authenticated synthetic GET/POST playback responses and media stream mapping using a fake FFprobe binary — GitHub Actions 39960435059 — full integration suite, format, Clippy and contracts passed
  - [ ] Unmodified Jellyfin clients accept media-source negotiation, codec selection and actual playback sessions
- 🛡️ **T307 — Opened-file descriptor validation for direct streaming** — `███████░░░ 67%`
  - [x] Validate Linux opened descriptor target inside canonical indexed root before streaming bytes — src/streaming.rs — file-handle confinement, Linux fd canonicalization and explicit non-Linux refusal
  - [x] CI proves symlink swapping, replaced library root and retained descriptor content invariants — GitHub Actions 39965980868 — format, Clippy, Rust streaming security regressions and contract checks passed
  - [ ] Linux production stress and adversarial multi-user filesystem security audit
- 🎵 **T308 — Authenticated static audio with HTTP byte ranges** — `███████░░░ 67%`
  - [x] Add Jellyfin-shaped Audio/{id}/stream GET and HEAD using validated direct-file handler and audio MIME — src/streaming.rs — video/audio shared descriptor-confined stream path and static-play restriction
  - [x] CI tests audio GET, HEAD, byte ranges, MIME, authentication and unsupported transcoding modes — GitHub Actions 39967422588 — full Rust audio/video HTTP tests, formatter, Clippy and ledger validators passed
  - [ ] Unmodified TV and mobile Jellyfin clients direct-play/seek real music and audiobook libraries
- ⏯️ **T309 — Guarded direct-play resume event persistence** — `███████░░░ 67%`
  - [x] Accept Jellyfin-shaped /Sessions/Playing start/progress/stop direct-play reports, preserve user-private resume ticks — src/playstate.rs; src/user_data.rs — validated item access and direct-only session event reporting
  - [x] CI proves position persistence, restart behavior, permission revocation, invalid ticks and rejection of unsupported remux/transcode methods — GitHub Actions 39982784834 — Rust playback report and user data integration tests passed
  - [ ] Unmodified TV clients demonstrate resume/reconnect, play thresholds, playback event delivery and full sessions/WebSocket behavior
- 🎞️ **T330 — Jellyfin container-suffixed direct-play GET/HEAD URLs** — `███████░░░ 67%`
  - [x] Support static /Videos/{id}/stream.ext and /Audio/{id}/stream.ext routes without lying about container conversion — src/streaming.rs — recognized stream.<container> URLs with exact indexed source extension guard
  - [x] CI verifies audio/video extension-path byte ranges, 404 invalid paths and 501 mismatched containers — GitHub Actions 39983627300 — Rust direct play named route tests, format and Clippy passed
  - [ ] Unmodified Jellyfin TV/mobile apps accept named stream URLs and seek in real formats
- 🚫 **T331 — Strict static streaming option negotiation boundaries** — `███████░░░ 67%`
  - [x] Refuse nonzero tick seeking, unsupported codec/subtitle selection, invalid source IDs and incompatible container queries — src/streaming.rs — guard unsupported transcode/remux negotiation fields and validate start tick bounds
  - [x] CI tests accurate 200/400/501 static stream option responses and regressions — GitHub Actions 39984094410 — Rust stream negotiation HTTP tests, Clippy and format passed
  - [ ] Full client codecs and remux/transcode negotiation validated with real FFmpeg, hardware and TV devices
- 🎧 **T332 — FFprobe audio playback sources and optional fallback permissions** — `███████░░░ 67%`
  - [x] Expose audio PlaybackInfo and a local direct-play URL; treat fallback flags as client permissions without falsely advertising transcode/remux support — src/playback.rs — audio files now return bounded FFprobe media streams, runtime, and the static audio route; direct-play-only capability truth
  - [x] CI verifies authenticated HTTP contract, representative fixtures and Rust quality gates — GitHub Actions 39986043745 — audio source, device fallback flags, permission grants, revocation and Clippy tests passed
  - [ ] Unmodified Jellyfin Smart TV clients validate this behavior with real libraries and metadata
- 🎚️ **T333 — Enriched Jellyfin media stream metadata from bounded FFprobe fields** — `███████░░░ 67%`
  - [x] Populate codec profile, frame rates, bit depth, pixel/color/audio layouts, text tags and subtitle dispositions only from supported FFprobe observations — src/playback.rs mapped_stream — bounded metadata strings and finite rational frame-rate checks
  - [x] CI validates HTTP MediaStream DTO mapping, invalid frame-rates and audio/subtitle synthetic fixtures — GitHub Actions 39990374327 — stream metadata HTTP tests, Rust formatting, Clippy and pinned parity inventory passed
  - [ ] Real Jellyfin TV codec negotiation and subtitle rendering conform across representative devices
- 🔐 **T334 — Jellyfin PlaybackInfo authenticated ownership and legacy query precedence** — `███████░░░ 67%`
  - [x] Enforce current user's ID in playback queries/body, apply deprecated query overrides, and reject nonzero seeks/unsupported stream selections explicitly — src/playback.rs — typed PascalCase request and camelCase query merge with identity and unsupported-mode guards
  - [x] CI verifies GET/POST user isolation, query override order, direct-play defaults, source selection and error status contracts — GitHub Actions 39992775206 — authenticated Rust HTTP integration, tests, Clippy and formatting passed
  - [ ] Unmodified Jellyfin clients verify playback negotiation across real capability profiles
- 📡 **T335 — Authenticated bounded Jellyfin bitrate-test transport** — `███████░░░ 67%`
  - [x] Implement size-validated GET /Playback/BitrateTest with non-cacheable random data streamed in bounded chunks and an exact Content-Length — src/bitrate.rs — direct Rust random-reader, no 100 MB response allocation or shell tooling
  - [x] CI validates authentication, default 102400 bytes, maximum 100000000, exact stream length, random payload and invalid-size responses — GitHub Actions 39994640474 — authenticated HTTP and bounded generator tests plus Clippy/format passed
  - [ ] Unmodified Smart TV clients verify playback bitrate estimation against real network conditions

### ⚙️ P10 — Transcoding and GPU — `░░░░░░░░░░ 0%`

- 🧱 **T310 — FFmpeg job controller** — `░░░░░░░░░░ 0%`
  - [ ] Implement process supervision, concurrency, cancellation and cleanup
  - [ ] Automated tests and Jellyfin baseline fixture verify process supervision, concurrency, cancellation and cleanup
  - [ ] Production or real-client acceptance validates process supervision, concurrency, cancellation and cleanup
- 📺 **T311 — HLS, DASH and transport** — `░░░░░░░░░░ 0%`
  - [ ] Implement segment manifests, adaptive profiles and timebase correctness
  - [ ] Automated tests and Jellyfin baseline fixture verify segment manifests, adaptive profiles and timebase correctness
  - [ ] Production or real-client acceptance validates segment manifests, adaptive profiles and timebase correctness
- 🖥️ **T312 — Linux GPU acceleration** — `░░░░░░░░░░ 0%`
  - [ ] Implement FFmpeg NVENC/NVDEC, VA-API and QSV capabilities with fallback
  - [ ] Automated tests and Jellyfin baseline fixture verify FFmpeg NVENC/NVDEC, VA-API and QSV capabilities with fallback
  - [ ] Production or real-client acceptance validates FFmpeg NVENC/NVDEC, VA-API and QSV capabilities with fallback
- 📈 **T313 — Playback telemetry** — `░░░░░░░░░░ 0%`
  - [ ] Implement active transcoding state, progress, bandwidth and resume reporting
  - [ ] Automated tests and Jellyfin baseline fixture verify active transcoding state, progress, bandwidth and resume reporting
  - [ ] Production or real-client acceptance validates active transcoding state, progress, bandwidth and resume reporting

### 💬 P11 — Subtitles and ancillary streams — `░░░░░░░░░░ 0%`

- 📝 **T320 — Subtitle delivery** — `░░░░░░░░░░ 0%`
  - [ ] Implement external/embedded tracks, fonts, formats and selection
  - [ ] Automated tests and Jellyfin baseline fixture verify external/embedded tracks, fonts, formats and selection
  - [ ] Production or real-client acceptance validates external/embedded tracks, fonts, formats and selection
- 🔥 **T321 — Subtitle burn-in** — `░░░░░░░░░░ 0%`
  - [ ] Implement FFmpeg subtitle overlay and TV-compatible fallback
  - [ ] Automated tests and Jellyfin baseline fixture verify FFmpeg subtitle overlay and TV-compatible fallback
  - [ ] Production or real-client acceptance validates FFmpeg subtitle overlay and TV-compatible fallback
- 🔊 **T322 — Audio pipelines** — `░░░░░░░░░░ 0%`
  - [ ] Implement codec fallback, remuxing, channel mapping and normalization
  - [ ] Automated tests and Jellyfin baseline fixture verify codec fallback, remuxing, channel mapping and normalization
  - [ ] Production or real-client acceptance validates codec fallback, remuxing, channel mapping and normalization
- 🖼️ **T323 — Trickplay and chapters** — `░░░░░░░░░░ 0%`
  - [ ] Implement preview sprites, chapter thumbnails, seeking and client metadata
  - [ ] Automated tests and Jellyfin baseline fixture verify preview sprites, chapter thumbnails, seeking and client metadata
  - [ ] Production or real-client acceptance validates preview sprites, chapter thumbnails, seeking and client metadata

## 📺 M4 — Advanced server features — `░░░░░░░░░░ 0%`

**Gate:** Music, live TV and collaboration features reach the same useful contract depth as Jellyfin.

### 📡 P12 — Live TV and recording — `░░░░░░░░░░ 0%`

- 📻 **T400 — Tuners and IPTV** — `░░░░░░░░░░ 0%`
  - [ ] Implement M3U, network tuners and live channel catalogs
  - [ ] Automated tests and Jellyfin baseline fixture verify M3U, network tuners and live channel catalogs
  - [ ] Production or real-client acceptance validates M3U, network tuners and live channel catalogs
- 🗓️ **T401 — Program guide** — `░░░░░░░░░░ 0%`
  - [ ] Implement EPG parsing, refresh, mapping and program search
  - [ ] Automated tests and Jellyfin baseline fixture verify EPG parsing, refresh, mapping and program search
  - [ ] Production or real-client acceptance validates EPG parsing, refresh, mapping and program search
- ⏺️ **T402 — DVR scheduler** — `░░░░░░░░░░ 0%`
  - [ ] Implement recordings, timers, conflicts, storage and cancellation
  - [ ] Automated tests and Jellyfin baseline fixture verify recordings, timers, conflicts, storage and cancellation
  - [ ] Production or real-client acceptance validates recordings, timers, conflicts, storage and cancellation

### 🎧 P13 — Rich media services — `░░░░░░░░░░ 0%`

- 🎶 **T410 — Music streaming** — `░░░░░░░░░░ 0%`
  - [ ] Implement audio endpoints, transcoding, instant mix and playlists
  - [ ] Automated tests and Jellyfin baseline fixture verify audio endpoints, transcoding, instant mix and playlists
  - [ ] Production or real-client acceptance validates audio endpoints, transcoding, instant mix and playlists
- ✍️ **T411 — Lyrics and collections** — `░░░░░░░░░░ 0%`
  - [ ] Implement synchronized lyrics, people/artists endpoints and smart queries
  - [ ] Automated tests and Jellyfin baseline fixture verify synchronized lyrics, people/artists endpoints and smart queries
  - [ ] Production or real-client acceptance validates synchronized lyrics, people/artists endpoints and smart queries
- 📚 **T412 — Books, photos and special items** — `░░░░░░░░░░ 0%`
  - [ ] Implement book/photo handling, thumbnails and browse behavior
  - [ ] Automated tests and Jellyfin baseline fixture verify book/photo handling, thumbnails and browse behavior
  - [ ] Production or real-client acceptance validates book/photo handling, thumbnails and browse behavior

### 🤝 P14 — Devices and synchronization — `░░░░░░░░░░ 0%`

- 👥 **T420 — SyncPlay** — `░░░░░░░░░░ 0%`
  - [ ] Implement group sessions, leader time and drift recovery
  - [ ] Automated tests and Jellyfin baseline fixture verify group sessions, leader time and drift recovery
  - [ ] Production or real-client acceptance validates group sessions, leader time and drift recovery
- 📲 **T421 — Chromecast and remote controls** — `░░░░░░░░░░ 0%`
  - [ ] Implement remote session commands and supporting device protocols
  - [ ] Automated tests and Jellyfin baseline fixture verify remote session commands and supporting device protocols
  - [ ] Production or real-client acceptance validates remote session commands and supporting device protocols
- 📺 **T422 — DLNA capability** — `░░░░░░░░░░ 0%`
  - [ ] Implement UPnP/SSDP and appropriate interoperability where offered by Jellyfin extensions
  - [ ] Automated tests and Jellyfin baseline fixture verify UPnP/SSDP and appropriate interoperability where offered by Jellyfin extensions
  - [ ] Production or real-client acceptance validates UPnP/SSDP and appropriate interoperability where offered by Jellyfin extensions

## 🛡️ M5 — Administration, extensibility and migration — `░░░░░░░░░░ 0%`

**Gate:** An operator can manage the entire server, retain their data and install equivalent supported extensions.

### 🛠️ P15 — Admin and accounts — `░░░░░░░░░░ 0%`

- 🧭 **T500 — Complete Rust-driven admin frontend** — `░░░░░░░░░░ 0%`
  - [ ] Implement admin features with responsive desktop/mobile navigation
  - [ ] Automated tests and Jellyfin baseline fixture verify admin features with responsive desktop/mobile navigation
  - [ ] Production or real-client acceptance validates admin features with responsive desktop/mobile navigation
- 👤 **T501 — User and permissions console** — `░░░░░░░░░░ 0%`
  - [ ] Implement users, access restrictions, password resets and sessions
  - [ ] Automated tests and Jellyfin baseline fixture verify users, access restrictions, password resets and sessions
  - [ ] Production or real-client acceptance validates users, access restrictions, password resets and sessions
- 🗃️ **T502 — Library and task console** — `░░░░░░░░░░ 0%`
  - [ ] Implement libraries, scanner, metadata jobs and system logs
  - [ ] Automated tests and Jellyfin baseline fixture verify libraries, scanner, metadata jobs and system logs
  - [ ] Production or real-client acceptance validates libraries, scanner, metadata jobs and system logs

### 🔌 P16 — Plugins and integrations — `░░░░░░░░░░ 0%`

- 🧩 **T510 — Rust-native extension host** — `░░░░░░░░░░ 0%`
  - [ ] Implement stable plugin ABI/API policy and lifecycle
  - [ ] Automated tests and Jellyfin baseline fixture verify stable plugin ABI/API policy and lifecycle
  - [ ] Production or real-client acceptance validates stable plugin ABI/API policy and lifecycle
- 🔁 **T511 — Built-in and first-party feature replacements** — `░░░░░░░░░░ 0%`
  - [ ] Implement required Jellyfin built-in or common plugin features without .NET
  - [ ] Automated tests and Jellyfin baseline fixture verify required Jellyfin built-in or common plugin features without .NET
  - [ ] Production or real-client acceptance validates required Jellyfin built-in or common plugin features without .NET
- 🌐 **T512 — Network and reverse proxy** — `░░░░░░░░░░ 0%`
  - [ ] Implement HTTPS, forwarded headers, CORS/CSRF rules and LAN discovery configuration
  - [ ] Automated tests and Jellyfin baseline fixture verify HTTPS, forwarded headers, CORS/CSRF rules and LAN discovery configuration
  - [ ] Production or real-client acceptance validates HTTPS, forwarded headers, CORS/CSRF rules and LAN discovery configuration

### 💾 P17 — Safety and migration — `░░░░░░░░░░ 0%`

- 📦 **T520 — Jellyfin data import** — `░░░░░░░░░░ 0%`
  - [ ] Implement users, watched state, metadata, playlists and configuration migration
  - [ ] Automated tests and Jellyfin baseline fixture verify users, watched state, metadata, playlists and configuration migration
  - [ ] Production or real-client acceptance validates users, watched state, metadata, playlists and configuration migration
- 🛟 **T521 — Backup/restore** — `░░░░░░░░░░ 0%`
  - [ ] Implement transactional backup, restore tests and reversible migration
  - [ ] Automated tests and Jellyfin baseline fixture verify transactional backup, restore tests and reversible migration
  - [ ] Production or real-client acceptance validates transactional backup, restore tests and reversible migration
- 🔐 **T522 — Security and operations** — `░░░░░░░░░░ 0%`
  - [ ] Implement secrets, audit trails, log redaction and startup-safe auth
  - [ ] Automated tests and Jellyfin baseline fixture verify secrets, audit trails, log redaction and startup-safe auth
  - [ ] Production or real-client acceptance validates secrets, audit trails, log redaction and startup-safe auth

## 🏁 M6 — Complete upstream server parity and MVP release — `░░░░░░░░░░ 0%`

**Gate:** All targeted Jellyfin v12.3 built-in C# server behavior is ported or explicitly resolved, and real clients pass a published matrix.

### 🗺️ P18 — Exhaustive upstream coverage — `░░░░░░░░░░ 0%`

- 🧾 **T600 — Map all upstream server modules** — `░░░░░░░░░░ 0%`
  - [ ] Implement version-pinned C# module and API inventory with owner/replacement mapping
  - [ ] Automated tests and Jellyfin baseline fixture verify version-pinned C# module and API inventory with owner/replacement mapping
  - [ ] Production or real-client acceptance validates version-pinned C# module and API inventory with owner/replacement mapping
- 🧮 **T601 — Full feature differential suite** — `░░░░░░░░░░ 0%`
  - [ ] Implement golden requests and responses, playback, metadata and settings parity
  - [ ] Automated tests and Jellyfin baseline fixture verify golden requests and responses, playback, metadata and settings parity
  - [ ] Production or real-client acceptance validates golden requests and responses, playback, metadata and settings parity
- 📖 **T602 — Document deliberate differences** — `░░░░░░░░░░ 0%`
  - [ ] Implement no undocumented or unverified omissions in port matrix
  - [ ] Automated tests and Jellyfin baseline fixture verify no undocumented or unverified omissions in port matrix
  - [ ] Production or real-client acceptance validates no undocumented or unverified omissions in port matrix
- 📐 **T603 — Performance and reliability parity** — `░░░░░░░░░░ 0%`
  - [ ] Implement stream throughput, memory/CPU, restarts and crash recovery
  - [ ] Automated tests and Jellyfin baseline fixture verify stream throughput, memory/CPU, restarts and crash recovery
  - [ ] Production or real-client acceptance validates stream throughput, memory/CPU, restarts and crash recovery

### 📺 P19 — Unmodified client acceptance — `░░░░░░░░░░ 0%`

- 📱 **T610 — Android TV / Fire TV** — `░░░░░░░░░░ 0%`
  - [ ] Implement unmodified native Jellyfin app pairing, browse, seek and playback
  - [ ] Automated tests and Jellyfin baseline fixture verify unmodified native Jellyfin app pairing, browse, seek and playback
  - [ ] Production or real-client acceptance validates unmodified native Jellyfin app pairing, browse, seek and playback
- 📡 **T611 — Roku, Samsung and LG** — `░░░░░░░░░░ 0%`
  - [ ] Implement unmodified Roku, Tizen and webOS Jellyfin clients pass acceptance
  - [ ] Automated tests and Jellyfin baseline fixture verify unmodified Roku, Tizen and webOS Jellyfin clients pass acceptance
  - [ ] Production or real-client acceptance validates unmodified Roku, Tizen and webOS Jellyfin clients pass acceptance
- 🌍 **T612 — Web and mobile clients** — `░░░░░░░░░░ 0%`
  - [ ] Implement supported web, Android and iOS client behavioral fixtures
  - [ ] Automated tests and Jellyfin baseline fixture verify supported web, Android and iOS client behavioral fixtures
  - [ ] Production or real-client acceptance validates supported web, Android and iOS client behavioral fixtures
- 🎬 **T613 — Codec/subtitle matrix** — `░░░░░░░░░░ 0%`
  - [ ] Implement direct play, remux, transcode, subtitle and hardware playback scenarios
  - [ ] Automated tests and Jellyfin baseline fixture verify direct play, remux, transcode, subtitle and hardware playback scenarios
  - [ ] Production or real-client acceptance validates direct play, remux, transcode, subtitle and hardware playback scenarios

### 🚀 P20 — Release gates — `░░░░░░░░░░ 0%`

- 📦 **T620 — Linux distribution** — `░░░░░░░░░░ 0%`
  - [ ] Implement native release binaries, package recipes, systemd and install/uninstall
  - [ ] Automated tests and Jellyfin baseline fixture verify native release binaries, package recipes, systemd and install/uninstall
  - [ ] Production or real-client acceptance validates native release binaries, package recipes, systemd and install/uninstall
- ⚖️ **T621 — Licensing and provenance audit** — `░░░░░░░░░░ 0%`
  - [ ] Implement file-by-file license/provenance and dependencies review
  - [ ] Automated tests and Jellyfin baseline fixture verify file-by-file license/provenance and dependencies review
  - [ ] Production or real-client acceptance validates file-by-file license/provenance and dependencies review
- 🔁 **T622 — Upgrade and rollback** — `░░░░░░░░░░ 0%`
  - [ ] Implement versioned upgrade, media-safety and rollback reproducibility
  - [ ] Automated tests and Jellyfin baseline fixture verify versioned upgrade, media-safety and rollback reproducibility
  - [ ] Production or real-client acceptance validates versioned upgrade, media-safety and rollback reproducibility
- 🏆 **T623 — Complete MVP signoff** — `░░░░░░░░░░ 0%`
  - [ ] Implement full-port acceptance checklist without unaccepted compatibility gaps
  - [ ] Automated tests and Jellyfin baseline fixture verify full-port acceptance checklist without unaccepted compatibility gaps
  - [ ] Production or real-client acceptance validates full-port acceptance checklist without unaccepted compatibility gaps

