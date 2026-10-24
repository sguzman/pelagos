# 🗺️ Pelagos full-port roadmap

**MVP means complete Jellyfin Server port**, not a slim movie streamer. Scope: [Jellyfin v12.3 baseline](SOURCE_BASELINE.md) plus compatibility with unmodified Smart TV Jellyfin clients. Every milestone M0 through M6 is part of MVP. Full source of truth: [progress.json](progress.json).

**Overall acceptance progress:** `████░░░░░░ 37%` across 164 atomic tasks. This tracks accepted checks, not lines translated or claimed runtime readiness. Checked source-path milestones are not substitutes for passing CI or real devices.

## 🧱 M0 — Foundation and project controls — `███░░░░░░░ 33%`

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

### 🛠️ P1 — Build and storage — `███░░░░░░░ 29%`

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
- ⚡ **T017 — Bounded batched per-user SQLite metadata retrieval** — `███████░░░ 67%`
  - [x] Replace per-item UserData reads with bounded indexed SQLite batches while preserving exact input ordering, duplicate IDs, missing defaults and private user partitioning — src/user_data.rs — <=400 item-id bindings per batch, user-scoped query and original-order reconstruction
  - [x] CI verifies hundreds of items across batch boundaries, watched/favorite/position/timestamp data, absent records, duplicate IDs and cross-user isolation — GitHub Actions 40012254173 — complete Rust tests, formatting, Clippy and repository guards passed
  - [ ] Large-scale library benchmarks and production performance/memory parity on Linux with unmodified Jellyfin client load

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

## 🔌 M1 — Jellyfin protocol and API parity — `█████░░░░░ 46%`

**Gate:** Unmodified Jellyfin clients can discover, authenticate, browse and maintain sessions without C#.

### 🌐 P3 — System and transports — `███░░░░░░░ 34%`

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
- 🌐 **T106 — Safe concrete listener URL in Jellyfin system identity** — `███████░░░ 67%`
  - [x] Advertise actual IPv4/IPv6 HTTP listener in System/Info/Public and authorized System/Info, keeping wildcard hosts unadvertised — src/app.rs; src/main.rs; tests/http.rs; tests/system_info_http.rs — concrete bind URL and matching system identity without Host trust
  - [x] CI verifies loopback, LAN IPv4, bracketed IPv6, wildcard suppression, hostile Host input and actual bound port plumbing — GitHub Actions 40004088369 — formatting, Clippy, all Rust tests, roadmap and upstream contract guards passed
  - [ ] Real Jellyfin web/TV app discovery and connection verifies advertised URL, proxy behavior and multi-interface host selection
- 🔑 **T107 — Explicit disabled Jellyfin Quick Connect signaling** — `███████░░░ 67%`
  - [x] Implement public pinned GET /QuickConnect/Enabled returning false and reject Initiate, Connect and authenticated Authorize without creating codes or sessions — src/auth_api.rs; tests/quick_connect_http.rs — disabled state and non-issuance of pairing credentials
  - [x] CI verifies public false boolean, 401 disabled operations with and without tokens, and no impacts on normal login — GitHub Actions 40004753528 — format, Clippy, full Rust tests and roadmap/upstream checks passed
  - [ ] Unmodified Jellyfin TV/web clients confirm disabled pairing fallback and complete enabled Quick Connect flow is implemented and verified

### 👥 P4 — Authentication and user state — `█████░░░░░ 48%`

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
- 👤 **T118 — Self-scoped Jellyfin user lookup by ID** — `███████░░░ 67%`
  - [x] Serve pinned GET /Users/{userId} for authenticated account only, matching /Users/Me without exposing other users — src/auth_api.rs; tests/user_lookup_http.rs — self lookup, inaccessible account 404, token validation
  - [x] CI verifies account equality, cross-account privacy and revoked token denial — GitHub Actions 39997998461 — formatting, Clippy, HTTP tests and source tracking passed
  - [ ] Unmodified Jellyfin clients validate full UserDto parity, admin identity permissions and user directory behavior
- 🔑 **T119 — Durable per-user original-media download permission independent of library grants** — `███████░░░ 67%`
  - [x] Implement persistent default-deny per-user EnableContentDownloading state, separate download CLI grant/revoke operations and permission-aware authenticated download requests — src/auth.rs; src/main.rs; src/streaming.rs; pinned Jellyfin v12.3 Policies.Download and PermissionKind.EnableContentDownloading
  - [x] Rust CI validates ordinary-account grant/revoke on active sessions, restart persistence, user-policy DTO, separate root authorization and original media downloads — GitHub Actions 40033786005: complete Rust integration/unit suites, Clippy, formatting, roadmap and pinned source/route checks passed
  - [ ] All pinned Jellyfin CanDownload user/item-policy cases and unmodified clients pass differential and production acceptance
- 👤 **T136 — Administrator-only password-backed Jellyfin account creation API** — `███████░░░ 67%`
  - [x] Implement authenticated administrator-only pinned POST /Users/New accepting strict Name/Password body, creating non-admin users with Argon2 password hashes and scoped User DTO subset — src/auth_api.rs create_account; src/auth.rs create_user; pinned Jellyfin v12.3 UserController.CreateUserByName and CreateUserByName DTO
  - [x] Rust CI verifies anonymous/user denial, privileged creation, password privacy, duplicate case-insensitive names, invalid and unknown privilege fields, restart and new-user login — GitHub Actions 40043498922: complete Rust HTTP tests, formatting, Clippy, pinned controller/contract and evidence ledger checks passed
  - [ ] Full Jellyfin UserDto fields, passwordless account lifecycle, user update/deletion/admin controls and unmodified client interoperability
- 📡 **T137 — Owned-session Jellyfin client capability reporting and durable metadata** — `███████░░░ 67%`
  - [x] Implement authenticated pinned POST /Sessions/Capabilities query subset with token-owned session mutation, bounded client fields, durable report and unimplemented remote-control capability left disabled — src/auth.rs set_session_capabilities/list_sessions with private SQLite FK persistence; src/auth_api.rs report_session_capabilities; pinned Jellyfin v12.3 SessionController.PostCapabilities
  - [x] Native HTTP CI validates unauthenticated/other-session denial, malformed media/command inputs, client metadata visibility, untouched sibling sessions, restart and token revocation — GitHub Actions 40047277534 — complete Rust unit and integration tests, formatting, Clippy, pinned controller/source and roadmap checks passed
  - [ ] Full ClientCapabilitiesDto/DeviceProfile support, real client capability negotiation, remote-control actions and unmodified TV acceptance
- 📺 **T138 — Full-form Jellyfin client capabilities JSON subset** — `███████░░░ 67%`
  - [x] Implement pinned POST /Sessions/Capabilities/Full with guarded own-session updates and parsed comma-delimited or JSON-array client media/command types; reject unsupported device profiles and links — src/auth_api.rs report_session_capabilities_full/from_capability_sequence/save_session_capabilities; src/auth.rs set_session_capabilities; pinned v12.3 SessionController.PostFullCapabilities and ClientCapabilitiesDto
  - [x] CI covers authenticated full-form request, unsupported DeviceProfile, cross-session denial, session metadata persistence and no fabricated remote-control support — GitHub Actions 40047568095 — full Rust formatting, Clippy, native HTTP/unit tests, roadmap ledger and pinned source/controller contract gates passed
  - [ ] DeviceProfile/URL metadata, complete upstream DTO validation, unmodified Jellyfin TV negotiation and actual remote-control/media functionality accepted
- 🕒 **T139 — Observed Jellyfin session recency filtering and LastActivityDate projection** — `███████░░░ 67%`
  - [x] Record authenticated request activity in a private cascading SQLite session table and expose last-activity UTC timestamps, custom device-name flags, and bounded GET /Sessions activeWithinSeconds filtering — src/auth.rs pelagos_session_activity/identify/list_sessions_with_activity; src/auth_api.rs sessions; pinned Jellyfin v12.3 SessionController.GetSessions and SessionInfoDto.LastActivityDate/HasCustomDeviceName
  - [x] Rust HTTP CI verifies aged-session exclusion, authenticated refresh, scoped device lookup, UTC session timestamps, persisted restart state, custom label flags, denied unsupported remote-control filter, and cascade deletion after logout — GitHub Actions 40053871853 — formatting, Clippy, full Rust test targets, roadmap and pinned source matrices passed
  - [ ] Upstream per-client heartbeat/activity semantics, remote-control filtering and unmodified Jellyfin clients accepted

### 🧭 P5 — General API contracts — `█████░░░░░ 50%`

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
- 🧬 **T128 — Permission-scoped Jellyfin item ancestor navigation** — `███████░░░ 67%`
  - [x] Implement pinned Items/{itemId}/Ancestors order with user ownership, accessible folder chain and UserData — src/catalog_api.rs; tests/ancestors_http.rs — direct-parent-first chain from the authorized item graph
  - [x] CI verifies ancestor ordering, root behavior, 401/403/404 responses, grants and immediate revocation — GitHub Actions 39997380191 — fmt, Clippy, Rust HTTP/unit tests, compatibility inventory and roadmap guard all passed
  - [ ] Unmodified Jellyfin TV/web clients validate ancestor navigation and full BaseItemDto behavior
- ⭐ **T129 — Authenticated Jellyfin UserData item reads and legacy route** — `███████░░░ 67%`
  - [x] Expose pinned UserItems/{itemId}/UserData GET and Users/{userId}/Items/{itemId}/UserData GET from private per-user SQLite state — src/user_data.rs; tests/user_data_http.rs — authorized current-user reads, independent from Jellyfin state
  - [x] CI verifies uninitialized state, watched/favorite persistence, cross-account denial and grant revocation — GitHub Actions 39997380191 — complete Rust CI passed with GET regressions
  - [ ] Unmodified Jellyfin clients verify read/write UserData DTO parity and synchronization semantics
- ✍️ **T130 — Transactional Jellyfin UserData item updates** — `███████░░░ 67%`
  - [x] Implement pinned POST UserItems/{itemId}/UserData and legacy route for supported played, favorite, play-count and resume-tick fields with strict unsupported-input handling — src/user_data.rs; tests/user_data_update_http.rs — private transactional SQLite updates with guarded field validation
  - [x] CI verifies atomic rejection, caller/library isolation, negative positions, persistent flags, idempotency and grant revocation — GitHub Actions 39998782291 — complete Rust CI passed on main
  - [ ] Unmodified Jellyfin clients validate full UpdateUserItemDataDto, preference fields, event synchronization and resume semantics
- ▶️ **T131 — Authorized persisted-position Jellyfin resume item queries** — `███████░░░ 67%`
  - [x] Implement pinned GET /UserItems/Resume and legacy user route from permitted video/audio media with position, filtering, pagination and playback activity timestamps — src/resume.rs; src/user_data.rs; src/app.rs; tests/resume_http.rs — filtered private resume listing and persisted positive-position timestamps
  - [x] CI verifies library revocation, cross-user denial, audio/video filters, paging, stable sorting, legacy lookup and explicit unsupported-option responses — GitHub Actions 40004888920 — full Rust CI and deterministic newest-first resume test with fixed SQLite activity timestamps passed
  - [ ] Unmodified Jellyfin client resume shelves, DatePlayed ordering, active-session exclusions and full metadata parity validated
- ↕️ **T133 — Validated item sorting, user scope and optional DTO flags** — `███████░░░ 67%`
  - [x] Accept pinned SortName and Ascending/Descending item queries, enableUserData/enableImages opt-outs, and self-scoped userId; refuse unsupported feature requests — src/catalog_api.rs; tests/items_options_http.rs — deterministic sort, omission flags, user identity and 501 refusals
  - [x] CI verifies descending pagination, user data/image opt-out, 401/403 account isolation and unimplemented sort/metadata rejection — GitHub Actions 40002822616 — full Rust CI and test suite green
  - [ ] Real Jellyfin clients validate multi-key sort/filter options, full DTO field semantics and query metadata parity
- 🔢 **T132 — Permission-scoped Jellyfin item counts with personal favorite filtering** — `███████░░░ 67%`
  - [x] Expose pinned GET /Items/Counts, recognized Movie/Series/Episode counts and indexed-media ItemCount without fabricating unsupported song/provider categories — src/catalog_api.rs — authenticated counts, physical indexed media, and deliberate zero values for nonclassified item kinds
  - [x] CI verifies unauthorized denial, cross-user 403, library grants/revocations, personal favorites, restart persistence, zero unsupported categories and admin scope — GitHub Actions 40010743895 — formatting, Clippy, full Rust HTTP/unit and tracking checks passed
  - [ ] Full Jellyfin ItemCounts category semantics, provider classifications, performance parity and unmodified-client acceptance verified
- 🎚️ **T134 — Recognized Movie and Series boolean filtering in catalog and search** — `███████░░░ 67%`
  - [x] Implement isSeries/isMovie in Items and Search/Hints, retain file-only default browse, expose explicit Series list while refusing unrelated classification flags — src/catalog_api.rs; src/search.rs; tests/classified_flags_http.rs — recognized categories remain tied to authorized catalog
  - [x] CI verifies mixed filter exclusion, explicit Series discovery, scoped Search/Hints, cross-user 403, unsupported flags and grant revocation — GitHub Actions 40011417762 — complete Rust tests, formatting, Clippy and contract/roadmap guards passed
  - [ ] Upstream boolean-filter combinations and actual unmodified Jellyfin TV clients validate full search/browse parity
- ♥️ **T135 — Personal watched and favorite filters across item browse and search** — `███████░░░ 67%`
  - [x] Implement pinned isPlayed/isFavorite boolean filters before pagination and counts with the authenticated account's persisted user state — src/catalog_api.rs; src/search.rs — batched user-state read before pagination and hideable UserData response
  - [x] Full CI verifies combined true/false filters, typed media search, search-folder distinctions, ordering, cross-user denial, restarts and library revocation — GitHub Actions 40016204498 — formatting, Clippy, full Rust unit/HTTP suites and source/roadmap checks passed
  - [ ] Unmodified Jellyfin clients validate inherited per-series played state, provider metadata interactions and complete upstream ItemQuery filter parity

## 📚 M2 — Full media library and metadata — `█████░░░░░ 46%`

**Gate:** Every core library type can be scanned, indexed, enriched, queried and updated like Jellyfin.

### 📂 P6 — Filesystem and items — `████░░░░░░ 37%`

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
- 📺 **T205 — Conservative local series, season and episode recognition** — `███████░░░ 67%`
  - [x] Recognize unambiguous season-folder-matched SxxEyy filenames with Unicode-aware paths while leaving unmatched media generic — src/naming.rs; tests/episode_items_http.rs — standalone parser and protected catalog DTO classification
  - [x] Full CI verifies stable SeasonId/SeriesId graph, partial naming metadata, image/search/resume integration and library grant revocation — GitHub Actions 40006848897 and 40007505990 — formatting, Clippy and full Rust HTTP/unit tests passed
  - [ ] Full Jellyfin naming, provider identifiers, collections, multi-episode files and unmodified client acceptance verified
- 🎬 **T206 — Folder-confirmed local movie title and year recognition** — `███████░░░ 67%`
  - [x] Recognize a conservative Title (YYYY) movie only when video filename and containing directory agree; otherwise retain generic Video without fabricated production metadata — src/naming.rs; src/catalog_api.rs — local movie hints, Movie DTO and optional ProductionYear
  - [x] CI verifies Movie type/isMovie filters, Search/Hints, resume persistence, stable IDs, Unicode titles and grant/revoke isolation — GitHub Actions 40008846700 — full formatting, Clippy, Rust HTTP/unit and source/roadmap checks passed
  - [ ] Unmodified Jellyfin clients verify full movie naming, NFO/remote metadata, editions, provider IDs, collections and media-source parity
- 🕒 **T207 — Durable first-indexed chronology and catalog schema v3 migration** — `███████░░░ 67%`
  - [x] Store immutable first-index timestamps on newly cataloged files, migrate v1/v2 histories conservatively and keep timestamps fixed during rescans — src/catalog.rs — schema v3 with last-seen backfill, dedicated latest index and timestamp-preserving conflict updates
  - [x] Full CI verifies initial ordering, deterministic ties, rescans, removal/readdition, backwards migration and forward-version safety — GitHub Actions 40015196184 — Rust formatting, Clippy, unit/integration suite and repository guards passed
  - [ ] Large-library migration benchmarking, known external creation-time reconciliation and Jellyfin upstream DateCreated compatibility are verified
- 🔄 **T208 — Administrator-triggered atomic refresh of registered media libraries** — `███████░░░ 67%`
  - [x] Implement privileged pinned Library/Refresh subset with registered-root-only read-only scans and one transaction for multi-library index changes — src/catalog.rs refresh_registered; src/catalog_api.rs refresh_library; pinned Jellyfin v12.3 LibraryController.RefreshLibrary
  - [x] Rust CI tests admin/anonymous authorization, new/stale media, registered roots only, offline all-or-none failure, redirected symlink refusal and restart — GitHub Actions 40039692258: cargo fmt, Clippy, full Rust unit/HTTP integration tests, pinned controller and roadmap checks passed
  - [ ] Asynchronous Jellyfin job semantics, cancellation, all operation failures, high-scale races and unmodified client acceptance verified

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

### 🎼 P8 — Library feature APIs — `█████░░░░░ 54%`

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
- 📦 **T227 — Indexed native container metadata on library items** — `███████░░░ 67%`
  - [x] Emit filename-derived media Container from indexed relative paths, omitting it from virtual library and folder items — src/catalog_api.rs; tests/catalog_http.rs — MP4/MKV/FLAC DTOs and private folder metadata
  - [x] CI verifies container DTO regression and full existing catalog suite — GitHub Actions 39998201271 — formatting, Clippy, catalog HTTP and full test checks passed
  - [ ] Unmodified Jellyfin clients and pinned BaseItemDto metadata differential validate container consistency
- 🌳 **T228 — Stable virtual user library root and connected folder navigation** — `███████░░░ 67%`
  - [x] Implement pinned Jellyfin GET /Items/Root and legacy root, with stable root ID and authorized collection-folder parent graph — src/catalog_api.rs; tests/root_http.rs; tests/ancestors_http.rs — stable pathless root, parent chain, recursive browse and self-only identity
  - [x] CI verifies empty/ungranted roots, grant/revoke, ancestor chain, direct/root DTO consistency and persistence across restart — GitHub Actions 40002822616 — full Rust format, Clippy, HTTP tests, upstream matrix and roadmap checks passed
  - [ ] Unmodified Jellyfin clients and upstream comparison verify root folder DTO and navigation parity
- 📚 **T229 — Jellyfin TV show season and episode navigation subsets** — `███████░░░ 67%`
  - [x] Implement pinned GET Shows/{seriesId}/Seasons and /Episodes with season filters, specials, index ordering, paging, StartItemId and optional UserData/images — src/shows.rs; tests/shows_http.rs — scoped catalog graph with explicit refusal of unsupported query options
  - [x] Full CI verifies unavailable/foreign series, cross-user identity, typed seasons/episodes, stable season links, continuation and permission revocation — GitHub Actions 40007768099 — formatting, Clippy, all Rust HTTP tests and upstream/roadmap checks passed
  - [ ] Unmodified Jellyfin Smart TV/web clients validate complete Shows routes, missing episodes, NextUp, metadata and alternate media behaviors
- 📁 **T230 — Confined original-media Jellyfin item file retrieval** — `███████░░░ 67%`
  - [x] Implement pinned GET/HEAD /Items/{itemId}/File using indexed, authorization-scoped opened descriptors, original bytes and MIME types with HTTP byte ranges — src/streaming.rs; tests/original_file_http.rs — shared Linux-confined direct-file reader for audio, video and standalone photos
  - [x] CI verifies unauthenticated 401, ungranted 404, cross-library protection, restart persistence, grant revocation, HEAD, partial/invalid ranges and swapped outside symlinks — GitHub Actions 40013990594 — full formatting, Clippy, Rust integration/unit tests and repository contract guards passed
  - [ ] Unmodified Jellyfin web/TV clients verify original-file handling, permissions and full upstream retrieval/download-policy differences
- 🆕 **T231 — Authorized ungrouped latest media shelf and legacy user route** — `███████░░░ 67%`
  - [x] Implement pinned GET /Items/Latest and legacy route with explicit groupItems=false, saved chronological order, recognized item filtering, per-user watched state and paging — src/latest.rs; tests/latest_http.rs — directory-scoped newest-first media, personal state, optional DTO flags and strict 501 on unsupported grouping
  - [x] CI verifies authorization, cross-user denial, revoked grants, stable rescan/restart ordering, movie/episode/photo media, played filters, limit=0 and legacy response — GitHub Actions 40015196184 — full Rust tests, formatting, Clippy, source/roadmap checks passed
  - [ ] Default grouped latest shelves, alternate Jellyfin library types, upstream DateCreated behavior and unmodified client acceptance verified
- 📺 **T232 — Pinned latest TV Episode-to-Series grouping** — `███████░░░ 67%`
  - [x] Group permitted latest episodes under a recognized Series only when two or more matching episodes are visible, report accurate ChildCount before group pagination, and retain single Episodes — src/latest.rs; src/catalog_api.rs — actual Series DTO container with optional ChildCount restricted to includeItemTypes=Episode
  - [x] CI verifies grouping defaults/type guards, chronological stable groups, played filtering, personal series state, limit boundaries, legacy lookup, restart and library revocation — GitHub Actions 40020602437 — full native Rust suite, formatting, Clippy, evidence/contract checks passed
  - [ ] Jellyfin music/photo album grouping, mixed library latest behavior and unmodified web/Smart TV acceptance verified
- ⏭️ **T233 — Scoped per-series Jellyfin NextUp watched progression** — `███████░░░ 67%`
  - [x] Implement pinned GET /Shows/NextUp for explicit accessible Series: first recognized unplayed episode after highest watched index, with resumable exclusion and paged query result — src/shows.rs; tests/next_up_http.rs — strict per-series chronology and refuse unsupported rewatching/cutoff/fields options
  - [x] CI verifies initial NextUp, advancing across seasons, skip earlier unplayed gaps, partial resume suppression, user-level permissions, restart persistence and library revocation — GitHub Actions 40021221370 — full formatting, Clippy, native HTTP/unit tests and upstream tracking gates passed
  - [ ] Multi-series automatic suggestions, missing/special episodes, alternate media versions, rewatching, cutoff and unmodified Jellyfin client acceptance verified
- 📡 **T234 — Private multi-series NextUp recommendations ordered by latest watched date** — `███████░░░ 67%`
  - [x] Discover only permitted recognized series with saved playback history, select each next episode after the highest watched position, and rank by personal last-played date with pagination — src/shows.rs — scoped series grouping using batched user-state and deterministic ranked QueryResult
  - [x] CI verifies multiple series with deterministic per-user playback dates, ignored unstarted series, independent account state, page boundaries, resumable exclusions, restart and library revocation — GitHub Actions 40021920924 — native Rust integration/unit tests, formatting, Clippy and pinned route/source/evidence guards passed
  - [ ] Full Jellyfin next-up date cutoff, alternate episode versions, specials, per-library exclusions, rewatching, and actual unmodified TV/web client acceptance
- 🗂️ **T235 — Library-root-scoped NextUp discovery with authorization checks** — `███████░░░ 67%`
  - [x] Resolve NextUp parentId solely to an authorized CollectionFolder and restrict candidate episodes through the local indexed parent graph; reject unsupported combined series filters and nested scopes — src/shows.rs — permission-scoped graph traversal and precise 404/501 failure boundaries
  - [x] CI verifies per-library NextUp results, cross-library isolation, inaccessible/unknown parents, nested unsupported scopes and post-revocation 404 — GitHub Actions 40023231676 — formatting, Clippy, new tests/next_up_many_http.rs coverage and complete Rust/contract suites passed
  - [ ] Jellyfin nested parent views, grouped user views, channels and unmodified TV clients accept complete upstream parentId semantics
- 📆 **T236 — Validated UTC last-watch cutoff for general NextUp recommendations** — `███████░░░ 67%`
  - [x] Support nextUpDateCutoff for general watched-series discovery with strict UTC parsing, real calendar validation and inclusive nanosecond-precision comparison — src/shows.rs — safe UTC key parser, explicit bad-date response, series-cutoff filtering before paging
  - [x] CI verifies exact cutoff, fraction boundary, invalid calendar days, leap dates, Unicode input safety, sorting and supported option combinations — GitHub Actions 40023231676 — dedicated parser tests and multi-series HTTP regressions alongside full Rust validation
  - [ ] Jellyfin offset-aware DateTime variations, alternate historical episode versions and unmodified web/Smart TV cutoff behavior verified
- 🎬 **T237 — Pinned grouped latest Movie, Episode and video-only shelf requests** — `███████░░░ 67%`
  - [x] Implement parent-container parity for video-only latest item types and permission-scoped default grouping without inventing audio/photo albums — src/latest.rs; pinned Jellyfin v12.3 BaseItem.LatestItemsIndexContainer, TV/Episode.cs and UserViewManager.cs
  - [x] Rust CI verifies grouped Movie/Episode/Video feeds, default video-only results, mixed-media refusal, ordering, count, legacy route and library permissions — GitHub Actions 40031375693: formatting, Clippy, complete Rust tests, roadmap and pinned contract checks passed
  - [ ] Unmodified Jellyfin Smart TV apps and production deployments accept latest Movie/Episode and default video-only shelves
- 🗃️ **T238 — Permission-scoped empty Jellyfin item BoxSet membership subset** — `███████░░░ 67%`
  - [x] Return pinned Jellyfin collection QueryResult for authorized indexed items, with truthful empty membership and fail-closed unsupported fields — src/catalog_api.rs; pinned Jellyfin v12.3 LibraryController.GetItemCollections; BoxSet indexing not yet implemented
  - [x] CI verifies admin and user authorization, missing/private IDs, paging, account isolation, restart and grant revocation — GitHub Actions 40031984883: full Rust test suite, formatting, Clippy, roadmap and pinned source/API contracts passed
  - [ ] Indexed real BoxSet memberships and unmodified Jellyfin TV client acceptance of item collections
- 📥 **T239 — Confined original-media Jellyfin Download transport and attachment headers** — `███████░░░ 67%`
  - [x] Implement authenticated original audio/video Download using Linux descriptor confinement, byte ranges, private caching and safe attachment filenames — src/streaming.rs; pinned Jellyfin v12.3 LibraryController.GetDownload and Policies.Download
  - [x] Rust CI verifies administrator/non-administrator permissions, supported MIME and byte ranges, Unicode disposition, sidecar/photo exclusion and outside-root symlink rejection — GitHub Actions 40033133007: cargo fmt, Clippy, complete Rust suite, roadmap and pinned protocol/source guards passed
  - [ ] Full Jellyfin Download permission policy, real-client transfers and production acceptance are demonstrated
- 🎵 **T240 — Scoped Jellyfin theme-media responses with pinned named local extras** — `███████░░░ 67%`
  - [x] Implement permission-scoped ThemeSongs, ThemeVideos, and ThemeMedia DTOs with recognized direct theme audio, theme-music and backdrops and pinned SortName ordering — src/theme.rs; src/catalog_api.rs; pinned Jellyfin v12.3 LibraryController, BaseItem.GetThemeSongs/GetThemeVideos and Emby.Naming NamingOptions.VideoExtraRules
  - [x] Rust HTTP CI covers empty and real theme-media DTOs, indexed audio/video playback, sort order, cross-user checks, library grants/revocation, restart and unsupported inheritance — GitHub Actions 40037734907: complete Rust unit/integration suite, formatting, Clippy, evidence-backed roadmap and pinned contract guards passed
  - [ ] Full pinned Jellyfin extras matching, provider metadata and unmodified client acceptance are demonstrated
- 🧬 **T241 — Permission-scoped inherited theme song and backdrop owner traversal** — `███████░░░ 67%`
  - [x] Implement per-kind pinned inheritFromParent traversal through authorized indexed parents with truthful separate ThemeSongs/ThemeVideos OwnerIds and loop protection — src/theme.rs; Jellyfin v12.3 LibraryController.GetThemeSongs/GetThemeVideos parent fallback
  - [x] Rust native HTTP regression verifies episode-to-series inherited song/video owners, direct-owner priority, empty ancestry, persisted catalog and existing authorization behavior — GitHub Actions 40038103381: formatting, Clippy, complete Rust suites, pinned source contracts and roadmap guards passed
  - [ ] Unmodified Jellyfin Smart TV clients and upstream differential tests validate inherited theme resolution across all metadata extras and parent configurations

## 🎞️ M3 — Playback, FFmpeg and hardware acceleration — `████░░░░░░ 35%`

**Gate:** Existing Jellyfin TV apps direct-play, remux, transcode and seek real media correctly.

### ▶️ P9 — Streaming engine — `█████░░░░░ 51%`

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
- 🛡️ **T336 — Strict malformed and repeated byte-range header rejection** — `███████░░░ 67%`
  - [x] Refuse opaque, unreadable, unfulfillable or duplicate HTTP Range headers without serving full media; preserve valid single-range streaming/download behavior — src/streaming.rs; Range header parsing and invalid_range Content-Range response
  - [x] Native Rust HTTP and unit CI regressions verify malformed/non-text and duplicated Range fields for original-media requests and downloads — GitHub Actions 40034407511: complete Rust test suite, formatting, Clippy, evidence-ledger and pinned controller/source contract checks passed
  - [ ] Upstream Jellyfin differential suite and unmodified real clients accept byte-range behavior across all supported files

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

## 🛡️ M5 — Administration, extensibility and migration — `████░░░░░░ 43%`

**Gate:** An operator can manage the entire server, retain their data and install equivalent supported extensions.

### 🛠️ P15 — Admin and accounts — `██████░░░░ 56%`

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
- 🗂️ **T503 — Administrator-only indexed media folders catalog** — `███████░░░ 67%`
  - [x] Expose pinned GET /Library/MediaFolders with administrator authorization and visible indexed empty or populated library roots — src/catalog_api.rs; tests/library_media_folders_http.rs — admin-bound read-only CollectionFolder query with no absolute paths
  - [x] CI verifies 401 unauthenticated, 403 viewer even after grants, hidden-filter subset, stable parent IDs, and empty roots — GitHub Actions 40003332448 — complete Rust CI and HTTP authorization tests passed
  - [ ] Real admin clients and pinned upstream Library/MediaFolders DTO/configuration semantics are verified
- 🔐 **T504 — Administrator-only Jellyfin library physical root paths** — `███████░░░ 67%`
  - [x] Implement pinned GET /Library/PhysicalPaths exposing only indexed source roots after administrator validation, separate from user-safe pathless DTOs — src/catalog_api.rs; tests/library_media_folders_http.rs — privileged root list read with sort/dedup and no ordinary-user path exposure
  - [x] CI verifies unauthenticated 401, nonadmin 403 even with library grants, indexed empty roots, exact source paths and persistent restart behavior — GitHub Actions 40012627288 — formatting, Clippy, native HTTP tests and source/roadmap guards passed
  - [ ] Unmodified Jellyfin administration clients verify exact library options, elevation semantics and end-to-end configuration workflows
- 🗃️ **T505 — Administrator-only read-only Jellyfin virtual media library inventory** — `███████░░░ 67%`
  - [x] Serve pinned GET /Library/VirtualFolders as a sparse VirtualFolderInfo array with stable ItemId, actual indexed Name and physical Locations; omit unknown collection/options metadata — src/catalog_api.rs; tests/virtual_folders_http.rs — admin-only read-only inventory with no fabricated library settings
  - [x] Full CI covers unauthenticated/non-admin denial, duplicate-free ordered roots, empty library persistence, identity parity with MediaFolders, rescan/restart stability and no ordinary-user path leakage — GitHub Actions 40023874352 — full Rust suite, formatting, Clippy, source contract and roadmap evidence checks passed
  - [ ] Library configuration mutation, CollectionType/LibraryOptions metadata, refresh state, first-run setup privileges and actual Jellyfin admin client acceptance verified
- 📁 **T506 — Administrator creation of a canonical one-root media library with independent display name** — `███████░░░ 67%`
  - [x] Implement administrator-only pinned VirtualFolders POST subset with one canonical existing root, independent UTF-8 display label, default options and atomic registered-root/label/index creation — src/catalog_api.rs create_virtual_folder; src/catalog.rs index_internal/register_named with v4 display_name; pinned Jellyfin v12.3 LibraryStructureController.AddVirtualFolder
  - [x] Rust CI verifies basename and custom-name creation, case-insensitive name conflicts, duplicate root denial, canonical/symlink checks, permission isolation, unsupported options, and persisted views — GitHub Actions 40043005135: full Rust unit/HTTP integration suites, formatting, Clippy, pinned route and roadmap checks passed
  - [ ] Concurrent high-scale registration, multi-path/library-options support, complete Jellyfin client naming semantics and unmodified administrator client acceptance
- 🧹 **T507 — Administrator virtual-library removal with durable grant cleanup** — `███████░░░ 67%`
  - [x] Implement administrator-only pinned VirtualFolders DELETE subset that atomically clears registered library rows and matching per-user grants across attached private SQLite stores without modifying media — src/catalog.rs unregister_by_name; src/catalog_api.rs remove_virtual_folder; pinned Jellyfin v12.3 LibraryStructureController.RemoveVirtualFolder
  - [x] Rust CI covers missing/offline roots, ambiguous names, anonymous/ordinary-account denial, source-media preservation, and deny-after-remove/re-add permissions — GitHub Actions 40041020127: complete Rust unit/HTTP suites, formatting, Clippy, evidence-ledger and pinned controller/contract gates passed
  - [ ] Full Jellyfin virtual-folder CRUD/options, crash recovery across filesystems, concurrent mutations and unmodified administrator clients accepted
- ✏️ **T508 — Persistent virtual library display-name changes without media moves or item ID churn** — `███████░░░ 67%`
  - [x] Implement pinned POST VirtualFolders/Name subset with v4 SQLite migration and atomic uniqueness validation; project renamed labels to admin inventory, library MediaFolders and user views without changing source paths or existing grants — src/catalog.rs LibraryEntry, rename_by_name and schema v4; src/catalog_api.rs rename_virtual_folder and accessible_nodes; pinned Jellyfin v12.3 LibraryStructureController.RenameVirtualFolder
  - [x] Rust CI verifies administrator-only rename, name conflicts, invalid paths, persistence/restart, unchanged physical paths and item IDs, old/new deletion semantics, existing grants and v3-to-v4 migration — GitHub Actions 40042434026: Rust unit and HTTP integration suites, cargo fmt, Clippy, roadmap and pinned upstream checks passed
  - [ ] Upstream Jellyfin metadata/ID rename equivalence, concurrent mutations, full admin UI and unmodified Smart TV client acceptance
- 👥 **T509 — Administrator-only Jellyfin users directory with disabled account filters** — `███████░░░ 67%`
  - [x] Read bounded user DTOs from the private account registry with administrator-only Users inventory, hidden/disabled filtering and no credential material — src/auth.rs list_users; src/auth_api.rs users; pinned Jellyfin v12.3 GET /Users contract
  - [x] Native HTTP regressions cover authenticated administrator directory access, ordinary-user denial, password/redacted DTO fields, disabled filter and restart — GitHub Actions 40044007701 and 40044882895: full Rust unit/HTTP tests, formatting, Clippy and pinned checks passed
  - [ ] Full Jellyfin Users DTO/filter parity and real unmodified client acceptance
- 🔐 **T513 — Administrator disabled-state user policy changes and immediate token revocation** — `███████░░░ 67%`
  - [x] Implement strict disabled-state subset of pinned POST Users/{userId}/Policy that refuses unsupported policy fields, protects administrators, and removes user sessions transactionally on disable — src/auth.rs set_account_disabled; src/auth_api.rs update_user_policy; pinned Jellyfin v12.3 UserController.UpdateUserPolicy
  - [x] Native HTTP suites verify anonymous/member denial, administrator protection, disabled login, immediate old-session revocation, re-enable without token resurrection and durable filters — GitHub Actions 40044766690 and 40044882895: formatting, Clippy, unit/HTTP suites and pinned gates passed
  - [ ] Complete Jellyfin policy controls, upstream differential behavior and unmodified real-client acceptance
- 🪪 **T514 — Administrator read-by-ID account lookup without exposing other accounts to ordinary users** — `███████░░░ 67%`
  - [x] Expose a bounded credential-free User DTO for privileged GET Users/{userId} while preserving ordinary self-only lookup, 404 denial and disabled-state visibility for administrators — src/auth.rs find_user; src/auth_api.rs user_by_id; pinned Jellyfin v12.3 UserController.GetUserById
  - [x] Native Rust HTTP checks verify self/current, privileged other-account lookup, unauthorized/missing requests, revocation, disabled account visibility, and no password material — GitHub Actions 40044882895: complete Rust unit/HTTP tests, Clippy, formatting, source matrix and evidence-backed checks passed
  - [ ] Full Jellyfin UserDto fields, authorization differential tests and unmodified TV/admin client acceptance
- ✏️ **T515 — Privileged account login-name editing without revoking identity or permissions** — `███████░░░ 67%`
  - [x] Implement bounded administrator-only POST Users/{userId} Name change with case-insensitive uniqueness, stable identity, retained password/session/grants, and rejection of unimplemented DTO fields — src/auth.rs rename_user; src/auth_api.rs update_user_name; pinned Jellyfin v12.3 UserController.UpdateUserLegacy
  - [x] Rust CI tests missing and duplicate account names, anonymous/member denial, unknown field rejection, existing-token consistency, new-name login, old-name denial, retained library grants, and restart — GitHub Actions 40045301553: complete Rust test suite, Clippy, formatting, pinned source and roadmap checks passed
  - [ ] Full upstream UserDto update semantics, concurrent administration and unmodified client acceptance
- 🔑 **T516 — Administrator ordinary-account password reset with session revocation** — `███████░░░ 67%`
  - [x] Implement restricted pinned POST Users/{userId}/Password with elevated account reset, Argon2 replacement and atomic invalidation of existing access tokens, while protecting administrator passwords — src/auth.rs reset_ordinary_user_password; src/auth_api.rs admin_reset_password; pinned Jellyfin v12.3 UserController.UpdateUserPassword
  - [x] Rust CI verifies anonymous/member denial, administrator protection, short password and unsupported reset-option rejection, token revocation, old/new credential behavior, stable account IDs and restart — GitHub Actions 40045690959: full Rust unit/HTTP suites, formatting, Clippy, source matrix and progress-ledger checks passed
  - [ ] Full Jellyfin UpdateUserPassword semantics, user current-password flows, recovery and unmodified real client acceptance
- 🔑 **T517 — Current-password-verified self-service account change with sibling-session revocation** — `███████░░░ 67%`
  - [x] Implement pinned POST /Users/Password self-account subset requiring CurrentPw and NewPw, with current-session preservation and other-session revocation in one authentication transaction — src/auth.rs change_own_password; src/auth_api.rs change_password; pinned Jellyfin v12.3 UserController.UpdateUserPassword
  - [x] Rust CI checks wrong/absent credentials, cross-account denial, forbidden passwordless reset, password length validation, admin self-change, revoked tokens, and old/new password login — GitHub Actions 40046475207 and 40046592852: full Rust HTTP/unit tests, cargo fmt, Clippy, roadmap and pinned contract gates passed; the latter includes transaction-time credential revalidation
  - [ ] Full Jellyfin UpdateUserPassword semantics, concurrent reset/login adversarial acceptance and unmodified Jellyfin client behavior verified
- ✏️ **T518 — Modern Jellyfin user rename endpoint using shared privileged account update** — `███████░░░ 67%`
  - [x] Implement modern pinned POST /Users optional userId subset with strict Name-only DTO and unified administrator-only rename handling — src/auth_api.rs update_user_name_modern/update_user_name_inner; src/auth.rs rename_user; pinned Jellyfin v12.3 UserController.UpdateUser
  - [x] Full Rust CI verifies missing/foreign account, non-admin and unauthenticated denial, unknown field rejection, stable user identity and token, old/new login and administrator self-rename — GitHub Actions 40047064155 — format, Clippy, Rust unit and HTTP integrations, ledger and pinned contract matrices passed
  - [ ] Full upstream UserDto/Configuration and self-preference semantics, differential concurrency and real unmodified Jellyfin client acceptance
- 📺 **T519 — Privileged persistent Jellyfin device directory and indexed info lookup** — `███████░░░ 67%`
  - [x] Register persistent client-supplied device IDs and implement elevated GET /Devices and /Devices/Info with token-redacted DTOs, live capability projection and observed authenticated device activity timestamps — src/auth.rs pelagos_devices/identify/list_registered_devices/registered_device_info; src/device_api.rs; Jellyfin v12.3 DevicesController; synthetic CI 40053871853
  - [x] Native HTTP suite verifies admin/ordinary/anonymous boundaries, deduplication, stable lookup, user filter, missing/unregistered IDs, token redaction, restart persistence, and active capability projection — GitHub Actions 40051326692, 40051766079, 40053871853 — Rust HTTP/unit suite, token redaction, stable lookup, activity time, restart and role boundaries verified
  - [ ] Full upstream device activity, numeric DTO IDs, metadata semantics and actual unmodified client compatibility accepted
- 🚪 **T523 — Administrator bounded multi-device logout with atomic session revocation** — `███████░░░ 67%`
  - [x] Implement pinned DELETE /Devices with up to 16 unique validated percent-decoded device IDs; prevalidate every registration then transactionally revoke matching sessions without changing unrelated sessions or stored device registrations — src/auth.rs revoke_registered_device_sessions; src/device_api.rs delete_device with RawQuery and form_urlencoded; pinned Jellyfin v12.3 DevicesController.DeleteDevice
  - [x] Native HTTP suite verifies anonymous/member denial, missing/unknown device IDs, invalid mixed-batch rollback, valid multi-device logout, percent-encoded IDs, surviving device records and new login after revocation — GitHub Actions 40052304486 and 40052337903 — full Rust unit/HTTP tests, formatting, Clippy, pinned source/controller matrices and roadmap gates passed
  - [ ] Unmodified Jellyfin admin and TV clients, concurrent device-session race cases and full upstream multi-device management acceptance
- ✏️ **T524 — Durable administrator device display-name override** — `███████░░░ 67%`
  - [x] Implement elevated GET/POST /Devices/Options including nullable or empty-string clearing, independent persistent display labels and stable numeric DeviceOptionsDto IDs without changing original device identity or token rights — src/auth.rs rename_registered_device/registered_device_options and pelagos_device_option_ids; src/device_api.rs update_device_options; CI 40053535666
  - [x] Native CI verifies role validation, unsupported body types, name update and clearing, stable numeric IDs, lazy mapping, repeated login, and persisted restart state — GitHub Actions 40051766079, 40052163479, 40053535666 — complete Rust formatting, Clippy, HTTP/unit, pinned matrices and roadmap checks passed
  - [ ] Unmodified Jellyfin device-management client acceptance and complete upstream options/UI semantics

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

