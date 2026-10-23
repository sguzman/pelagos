# 📺 Compatibility contract

**Required MVP:** a native Rust replacement for Jellyfin Server v12.3 that works with **unmodified Jellyfin applications on Smart TVs**. No custom television app is an acceptable substitute for this gate.

| Client | Discovery / connect | Login / Quick Connect | Browse / artwork | Direct play / seek | Remux/transcode / subtitles | Resume / remote session |
| --- | --- | --- | --- | --- | --- | --- |
| Jellyfin Android TV / Fire TV | Not tested | Not tested | Not tested | Not tested | Not tested | Not tested |
| Jellyfin LG webOS | Not tested | Not tested | Not tested | Not tested | Not tested | Not tested |
| Jellyfin Samsung Tizen | Not tested | Not tested | Not tested | Not tested | Not tested | Not tested |
| Jellyfin Roku | Not tested | Not tested | Not tested | Not tested | Not tested | Not tested |
| Jellyfin web/mobile | Not tested | Not tested | Not tested | Not tested | Not tested | Not tested |

**Test policy:** record specific app version, device model/runtime, Jellyfin Server baseline, request/response fixtures, playback logs and acceptance evidence (synthetic/open media only). Evaluate HTTP status/JSON shapes, headers, authentication tokens, image paths, WebSocket events, device profiles, content headers, HTTP ranges, HLS manifests, progress sync and seeking. Compare equivalent sessions against the pinned upstream server, not guessed API expectations.

**Current state:** Pelagos has synthetic-tested authentication, preliminary Jellyfin system/item/user APIs, catalog and artwork endpoints, and guarded static audio/video streaming. None of this establishes actual connection, login, browsing or playback in an unmodified Jellyfin TV app. The TV apps remain the final acceptance authority.

## Discovery scaffold evidence

Rust-based UDP v12.3 auto-discovery now exists and passes loopback network tests ([CI 39954060555](https://github.com/sguzman/pelagos/actions/runs/39954060555)). It is disabled by default and can only advertise a deliberately LAN-bound HTTP listener. All actual Smart TV app discovery cells above remain **Not tested** until a real, unmodified client is observed. Local password authentication and guarded static direct-file streaming now exist, but have **not** passed real-client acceptance.

## Authentication and folder navigation scaffolds

Local password login, token authorization and logout now pass synthetic HTTP integration tests ([CI 39956568616](https://github.com/sguzman/pelagos/actions/runs/39956568616)). The first read-only Jellyfin-shaped `/Items`, `/UserViews` and `parentId` navigation routes are also automated-test green ([CI 39957906828](https://github.com/sguzman/pelagos/actions/runs/39957906828)). None of these runs used an actual Jellyfin Smart TV app or a running Jellyfin comparator. All acceptance-matrix cells remain **Not tested**.

## Initial direct-file video streaming

A synthetic, admin-token-only streaming test now passes for `GET/HEAD /Videos/{id}/stream?static=true`, byte ranges and symlink-root confinement ([CI 39958988196](https://github.com/sguzman/pelagos/actions/runs/39958988196)). All unmodified Smart TV playback matrix cells remain **Not tested**. This behavior is only one piece of the complete playback protocol and no user should interpret it as validated client compatibility.

## Media-source playback information and empty-library roots

An administrator can request synthetic-verified `GET/POST /Items/{id}/PlaybackInfo` with FFprobe-derived media streams and conservative direct-play flags ([CI 39960435059](https://github.com/sguzman/pelagos/actions/runs/39960435059)). Empty catalog library roots persist through schema v2 migration and appear in `/UserViews` and empty parent queries ([CI 39960935517](https://github.com/sguzman/pelagos/actions/runs/39960935517)). These are internal contract checks, not real-device playback acceptance. All Smart TV compatibility matrix gates remain **Not tested**.

## Device-aware sessions (synthetic evidence only)

Token issuance creates a real persistent session record and login returns `SessionInfo` with client/device fields. Authenticated `GET /Sessions` can filter by device and is scoped to the owner or administrator; remote-control and activity filters remain unsupported. Synthetic REST and persistence tests passed [CI 39962550684](https://github.com/sguzman/pelagos/actions/runs/39962550684). Actual unmodified TV login and WebSocket/reconnect behavior is **Not tested**.

## Root permission scope and descriptor confinement

Pelagos supports explicit root-level media read grants for ordinary accounts, while administrators see all indexed libraries. Routes for library views, item discovery, PlaybackInfo and direct-file streaming enforce the same deny-by-default permission set, with synthetic authorization coverage ([CI 39965370233](https://github.com/sguzman/pelagos/actions/runs/39965370233)). The Linux stream handler verifies the opened file descriptor to avoid symlink-swap redirection ([CI 39965980868](https://github.com/sguzman/pelagos/actions/runs/39965980868)). Real Jellyfin TV-client access-control and codec acceptance tests remain **Not tested**.

## Filename search and direct audio

Jellyfin-shaped `/Search/Hints` results now provide read-permission-scoped basic name search, recognized item/media filters and pagination ([CI 39967003914](https://github.com/sguzman/pelagos/actions/runs/39967003914)). Authenticated `/Audio/{id}/stream?static=true` adds raw audio GET/HEAD and byte ranges ([CI 39967422588](https://github.com/sguzman/pelagos/actions/runs/39967422588)). These are contract-shaped diagnostic tests, **not** verified behavior in an unmodified Jellyfin Smart TV client. All real-client acceptance gates remain **Not tested**.

## Shared item/media filters

The initial `/Items` and `/Search/Hints` implementations share a recognized media/item type filter layer ([CI 39968255232](https://github.com/sguzman/pelagos/actions/runs/39968255232)). This is synthetic HTTP evidence only; advanced Jellyfin filters and actual TV browsing remain **Not tested**.

## Personal media state and direct-play resume (synthetic only)

Per-user played/favorite routes, legacy user-item lookup and read-only `UserData` item responses pass Rust/HTTP acceptance fixtures ([CI 39982107696](https://github.com/sguzman/pelagos/actions/runs/39982107696)). Guarded direct-play start/progress/stop routes persist per-account `PositionTicks` and reject unsupported transcoding variants ([CI 39982784834](https://github.com/sguzman/pelagos/actions/runs/39982784834)). These are **not** proof of unmodified TV resume, offline sync, completion thresholds or real playback session behavior. Real-device compatibility remains **Not tested**.

## Container-named streams and negotiating unsupported codecs

The initial static direct-play handler accepts `/Videos/{id}/stream.mp4` and `/Audio/{id}/stream.mp3` when the indexed container matches, including HTTP ranges ([CI 39983627300](https://github.com/sguzman/pelagos/actions/runs/39983627300)). Unsupported explicit remux/transcode/tick-seek requests return 501 rather than false 200 success ([CI 39984094410](https://github.com/sguzman/pelagos/actions/runs/39984094410)). Unmodified TV app playback remains **Not tested**.

## Local primary artwork and audio PlaybackInfo

Direct-play-only audio `PlaybackInfo` reports now accept permissive fallback capability flags but never claim transcoding or remuxing ([CI 39986043745](https://github.com/sguzman/pelagos/actions/runs/39986043745)). Local, indexed primary poster and folder art is advertised and can be fetched via Jellyfin `Primary` and `Primary/0` URLs ([CI 39987262612](https://github.com/sguzman/pelagos/actions/runs/39987262612), [CI 39988010040](https://github.com/sguzman/pelagos/actions/runs/39988010040)). The `/Items/{id}/Images` list exposes local ImageInfo metadata without filesystem paths ([CI 39988687987](https://github.com/sguzman/pelagos/actions/runs/39988687987)). Image resizing, richer metadata providers and unmodified TV rendering remain **Not tested**.

## Sidecar browse consistency and playback request identity

Media covers no longer populate unrelated Photo browsing/search results, while standalone images remain visible ([CI 39989870345](https://github.com/sguzman/pelagos/actions/runs/39989870345)). Probe-driven MediaStream mapping supports additional codec/color/profile/frame-rate fields ([CI 39990374327](https://github.com/sguzman/pelagos/actions/runs/39990374327)). Authenticated `PlaybackInfo` now supports Jellyfin query-over-body precedence and enforces `userId`, with guarded unsupported negotiation fields ([CI 39992775206](https://github.com/sguzman/pelagos/actions/runs/39992775206)). These remain synthetic evidence: **unmodified TV acceptance not yet tested**.

## Private artwork revalidation

Primary images now support per-user scoped conditional ETag fetches and empty 304 responses; the validator changes when original size/mtime changes without a library rescan ([CI 39993608914](https://github.com/sguzman/pelagos/actions/runs/39993608914)). **Unmodified TV client not tested.**

## Bandwidth test API

Authenticated `GET /Playback/BitrateTest?size=N` now returns non-cacheable random data of the exact allowed requested length, without large buffering ([CI 39994640474](https://github.com/sguzman/pelagos/actions/runs/39994640474)). **Smart TV behavior not yet tested**.

## Authenticated system information

`GET /System/Info` now returns a private, authenticated Jellyfin-shaped subset consistent with public server identity, without claiming WebSockets, monitoring or self-restart, and without leaking state directory paths ([CI 39995545838](https://github.com/sguzman/pelagos/actions/runs/39995545838)). Real TV handshake remains **Not tested**.

## Scoped ancestor navigation and direct UserData GET (synthetic only)

The pinned `/Items/{itemId}/Ancestors` response returns the immediate parent first, followed by its authorized parent chain, including a visible library root. New `/UserItems/{itemId}/UserData` and legacy `/Users/{userId}/Items/{itemId}/UserData` GET endpoints read independently stored user state. [CI 39997380191](https://github.com/sguzman/pelagos/actions/runs/39997380191) verifies authentication, cross-user isolation, 404 for inaccessible media, order, defaults and grant revocation. Real Jellyfin clients, DTO parity and third-party metadata remain **Not tested**.

## Account lookup, container DTOs and user-data write subset (synthetic only)

The self-scoped `/Users/{userId}` route passes [CI 39997998461](https://github.com/sguzman/pelagos/actions/runs/39997998461); other users' profiles remain hidden rather than emulating Jellyfin's full user-directory policy. Indexed native media containers are exposed in `BaseItemDto` subsets with [CI 39998201271](https://github.com/sguzman/pelagos/actions/runs/39998201271). `POST /UserItems/{itemId}/UserData` and the legacy route persist guarded partial updates of watched/favorite state, play counts and positions, while explicitly refusing unsupported fields; authorization, persistence and rejection tests pass [CI 39998782291](https://github.com/sguzman/pelagos/actions/runs/39998782291). None of these endpoints has yet passed real TV-client or full upstream DTO differential acceptance.

## Resume item discovery (synthetic only)

Pelagos now implements preliminary pinned `GET /UserItems/Resume` and legacy `GET /Users/{userId}/Items/Resume` using accessible, indexed audio/video items and independently persisted positive playback ticks. The Rust suite confirms personal state isolation, search/media filtering, paging, grant revocation and activity timestamps ([CI 39999748396](https://github.com/sguzman/pelagos/actions/runs/39999748396)). Full Jellyfin resume ordering, completed playback thresholds, active-session filtering, alternate versions and unmodified client acceptance remain **Not tested or unimplemented**.

## Stable virtual root and item query option subset (synthetic only)

An authenticated virtual user root is now available through Jellyfin's pinned `/Items/Root` and legacy `/Users/{userId}/Items/Root`. Its ID is stable across restarts; its authorized collection children and item ancestor chains are connected, with no source paths exposed. The `/Items` browse subset accepts SortName and descending order, scoped `userId`, and `enableImages=false` / `enableUserData=false` flags; unsupported sort names and unavailable extra DTO field projections explicitly return 501. Permission, paging and identity tests passed [CI 40002822616](https://github.com/sguzman/pelagos/actions/runs/40002822616). No unmodified Jellyfin TV client or full pinned DTO comparison has validated this behavior.

## Administrator media-folder listing (synthetic only)

`GET /Library/MediaFolders` now requires an administrator account and lists only currently indexed, non-hidden Pelagos collection roots (including empty libraries), with no absolute path values in item DTOs. The existing Pelagos catalog does not implement Jellyfin's hidden or disabled library configuration. Cross-account authorization, empty-library and filter regression tests passed [CI 40003332448](https://github.com/sguzman/pelagos/actions/runs/40003332448); no unmodified Jellyfin admin UI or pinned-server comparison has validated full behavior.

## Advertised concrete HTTP identity (synthetic only)

Jellyfin-shaped public and authenticated system information now report the actual configured listener URL when Pelagos binds to a concrete IPv4 or IPv6 address, including its chosen port. Wildcard binds do not advertise unusable `0.0.0.0`/`::` addresses; hostile HTTP `Host` headers cannot affect this value. The auth/public field agreement, network URI shapes and safe fallbacks passed [CI 40004088369](https://github.com/sguzman/pelagos/actions/runs/40004088369). Real LAN discovery, client-selected remote URLs and proxy/multi-interface compatibility remain **Not tested**.

## Explicit disabled Quick Connect contract and resume ordering (synthetic only)

Pelagos reports Quick Connect disabled at the pinned `GET /QuickConnect/Enabled` endpoint and refuses its three pairing operations without generating secrets or codes ([CI 40004753528](https://github.com/sguzman/pelagos/actions/runs/40004753528)). This supports truthful capability signaling, **not** an enabled Quick Connect feature or actual TV pairing. Separately, a deterministic SQLite-fixture test now verifies newest-first resumable items by persisted playback activity, including ordering when UserData is suppressed ([CI 40004888920](https://github.com/sguzman/pelagos/actions/runs/40004888920)). Full client validation, date semantics and session exclusions remain open.

## Conservative TV catalog and Shows query routes (synthetic only)

Recognized single SxxEyy names under matching season folders produce Series/Season/Episode DTOs, numerical indices, and stable SeriesId/SeasonId links; other video files remain generic unless recognized ([CI 40006848897](https://github.com/sguzman/pelagos/actions/runs/40006848897)). Permission-scoped pinned `GET /Shows/{seriesId}/Seasons` and `/Episodes` subsets now provide season/special filters, page slices, startItemId continuation, partial UserData and defensively rejected unsupported query options ([CI 40007768099](https://github.com/sguzman/pelagos/actions/runs/40007768099)). These are **not** a complete Emby.Naming implementation, upstream Jellyfin metadata graph, NextUp/missing-episode engine or verified real TV-client API.

## Folder-confirmed movie recognition (synthetic only)

Pelagos recognizes Movie DTOs only for a matching local `Title (YYYY)` video file inside a `Title (YYYY)` directory, exposing the parsed title and optional production year without guessing at generic videos. Catalog `includeItemTypes=Movie` and `isMovie` queries, Search/Hints Movie filtering, personal resume data and library grant revocation are verified by [CI 40008846700](https://github.com/sguzman/pelagos/actions/runs/40008846700). Broader Jellyfin movie naming conventions, provider/NFO metadata, alternate editions and unmodified client acceptance remain **Not tested or unimplemented**.

## ItemCounts and recognized type filtering (synthetic only)

Pinned `GET /Items/Counts` is available under personal library authorization and supports `userId` self-checks and `isFavorite` from persisted per-user state. The present `ItemCount` covers visible indexed media files; MovieCount, SeriesCount and EpisodeCount use conservative filename-derived classifications; other Jellyfin ItemCounts categories remain zero because generic audio/video is not promoted to songs, books, etc. ([CI 40010743895](https://github.com/sguzman/pelagos/actions/runs/40010743895)). `isMovie` and `isSeries` are now implemented in both item browse and search hints, with default folder exclusion maintained ([CI 40011417762](https://github.com/sguzman/pelagos/actions/runs/40011417762)). Full upstream metadata counts, combined filters and unmodified client acceptance remain open.

## Batched private user state and privileged source path inventory (synthetic only)

Pelagos' persisted playback/favorite UserData lookup now uses chunked parameterized SQLite requests, preserving caller ordering and missing defaults while avoiding one SQL query per item. A 900+ item test covers bind boundaries and per-user isolation ([CI 40012254173](https://github.com/sguzman/pelagos/actions/runs/40012254173)). Pinned `GET /Library/PhysicalPaths` returns currently indexed on-disk library root paths only for administrators; ordinary viewers get 403 even if granted a library and untrusted callers get 401. Item DTOs remain pathless; admin authorization, roots and restart tests passed [CI 40012627288](https://github.com/sguzman/pelagos/actions/runs/40012627288). Broader Jellyfin elevation policy and production performance/client acceptance remain open.

## Original indexed item file retrieval (synthetic only)

Pinned Jellyfin `GET /Items/{itemId}/File` (and HEAD) now serves the permitted indexed original audio/video/photo, retaining actual container bytes and HTTP single-byte-range semantics. It uses the same Linux file-descriptor confinement path as direct streaming, including denial of a post-index symlink pointing outside a library. Cross-user/library access, revocation, MIME types, 416 range behavior and HEAD passed [CI 40013990594](https://github.com/sguzman/pelagos/actions/runs/40013990594). The separate Jellyfin `/Items/{itemId}/Download` policy, broader file permissions and unmodified clients are **not** implemented/validated by this subset.

## Durable latest-media chronology and ungrouped query (synthetic only)

Pelagos schema v3 stores first-indexed timestamps independently of last-seen scan timestamps. It retains these across rescans, backfills legacy v1/v2 rows with their best-known last-seen time and gives re-added files a fresh index time. Pinned `GET /Items/Latest` and `/Users/{userId}/Items/Latest` now implement a strict **ungrouped** subset: `groupItems=false` is required, with ordering, type filters, parent scope, personal played filtering, limit, images and UserData flags. Default/explicit `groupItems=true` and unsupported advanced projections return 501 instead of fabricating grouped folders. Migration, cross-user authorization, sorting, restart/revocation and legacy-route tests passed [CI 40015196184](https://github.com/sguzman/pelagos/actions/runs/40015196184). Full Jellyfin latest grouping, upstream DateCreated equivalence, provider metadata and real clients remain open.

## Personal item watched and favorite query filters (synthetic only)

Pelagos now supports `isPlayed=true|false` and `isFavorite=true|false` in `/Items` and `/Search/Hints`, applying each account's saved flags before count and pagination. Full user data can be excluded from returned item DTOs while filters still work. A typed search distinguishes generic collection folders from media, and unauthorized accounts cannot inspect another user's filter state. Regression coverage for combinations, page boundaries, post-restart data and grant revocation passed [CI 40016204498](https://github.com/sguzman/pelagos/actions/runs/40016204498). Inherited series/season watched aggregation and broader Jellyfin query/provider parity remain unimplemented.

## TV latest Episodes grouped under Series (synthetic only)

In pinned Jellyfin v12.3, `Episode.LatestItemsIndexContainer` is the Series. Pelagos now handles that subset when the caller specifies only `includeItemTypes=Episode`; default/explicit true `groupItems` uses authorized Series DTO containers for two or more matching episodes, emits the actual counted `ChildCount`, and preserves a single Episode as an Episode. `limit` applies to completed groups, and watched filtering precedes grouping. Private user data belongs to the selected Episode or Series, not another account. CI [40020602437](https://github.com/sguzman/pelagos/actions/runs/40020602437) verifies these semantics, revocation, persistence and legacy requests. Grouping for song/photo albums and real unmodified Jellyfin clients remain **unimplemented/unverified**.

## Explicit-Series TV NextUp progression (synthetic only)

Pelagos implements a deliberately scoped `GET /Shows/NextUp?seriesId={recognizedSeriesId}` using the pinned manager's season/episode index ordering and persisted per-user watched flags. It chooses the first unplayed, non-special local episode strictly after the highest watched episode; if there is no watch history, it starts at the earliest recognized episode. It can suppress resumable selections, paginate the zero-or-one result, omit images or user data and reject cross-user or ungranted series access. Requests without seriesId, or with unsupported rewatching, cutoff and rich metadata options, return 501 rather than pretending to implement the full algorithm. Test coverage passed [CI 40021221370](https://github.com/sguzman/pelagos/actions/runs/40021221370). General multi-series candidate ordering, specials, alternate media versions, rewatching and unmodified-client checks remain open.

## Multi-series watched NextUp feed (synthetic only)

Without `seriesId`, Pelagos now implements a selective form of pinned Jellyfin v12.3 NextUp: the authenticated user's visible recognized series must have a recorded watched episode and last-played timestamp, then the next unplayed episode after the highest watched index is selected. Series candidates are ordered by that user's latest playback timestamp, followed by stable IDs; pagination, optional UserData/images and `enableResumable=false` filtering remain scoped. Explicit `seriesId` still allows first-episode discovery with no prior watch history. Synthetic integration coverage for distinct series, recency, startup, isolated user data, reboot, revocation and resumable suppression passed [CI 40021920924](https://github.com/sguzman/pelagos/actions/runs/40021920924). Advanced Jellyfin media variants, specials, rewatching, date cutoff, excluded libraries and genuine unmodified TV clients remain unsupported/unverified.

## NextUp library parent and UTC cutoff (synthetic only)

The pinned `Shows/NextUp` query now supports a `parentId` restricted to visible `CollectionFolder` roots. Media descendants are checked against the per-request authorized item graph, so inaccessible libraries are not disclosed; unknown/revoked roots yield 404. Nested folders and parent-plus-series intersections yield 501 until true upstream semantics are implemented. General multi-series requests also accept `nextUpDateCutoff` as a UTC `YYYY-MM-DDTHH:MM:SS[.fraction]Z`, with calendar validity checks, fractional-second comparison, and inclusive filtering before pagination. The query rejects invalid dates with 400 and does not fake offset-aware input support. CI [40023231676](https://github.com/sguzman/pelagos/actions/runs/40023231676) verifies privacy, revoked grants, time boundaries and parser safety. Real TV clients, channels, nested views and other DateTime forms remain unverified.

## Administrator virtual-library inventory (synthetic only)

`GET /Library/VirtualFolders` now offers an administrator-only, read-only subset of the pinned `VirtualFolderInfo` API: stable `ItemId` consistent with `/Library/MediaFolders`, root directory basename `Name`, and canonical indexed physical `Locations`. Empty indexed libraries remain listed, and non-admin or unauthenticated accounts cannot retrieve disk paths. `CollectionType`, `LibraryOptions`, artwork, refresh state, and write operations are absent rather than invented. Coverage for role restrictions, identity, restart, rescans and path isolation passed [CI 40023874352](https://github.com/sguzman/pelagos/actions/runs/40023874352). First-run elevated policy and real Jellyfin administration clients remain unverified.

## Grouped Movie and permission-scoped default video shelves (synthetic only)

Pinned Jellyfin v12.3 returns each Movie and generic Video without a latest parent container, but groups Episodes under Series when two or more are present. Pelagos now accepts explicit grouped `includeItemTypes` combinations drawn from `Movie,Episode,Video`; Movie/Video items stay separate and Episodes retain their chronological Series grouping, with truthful `ChildCount` before final result truncation. An absent or empty type filter is accepted for default grouping **only when every visible indexed media item is video**. The test deliberately indexes a separate private audio library: another account cannot infer its contents through default grouping, and when permission is explicitly granted, grouped default returns 501 because MusicAlbum is not modeled. Photo and Audio grouping requests similarly remain unsupported, while `groupItems=false` remains available. Synthetic native HTTP regressions, permission revocation, legacy paths and all CI gates passed [CI 40031375693](https://github.com/sguzman/pelagos/actions/runs/40031375693). **No unmodified Jellyfin Smart TV client has accepted these shelves yet.**

## Authorized empty item collection memberships (synthetic only)

The pinned Jellyfin v12.3 `GET /Items/{itemId}/Collections` retrieves BoxSets containing an item. Pelagos does not model BoxSet entities or their memberships, so its present subset returns an authentic zero-count `QueryResult` for an existing, authorized indexed item, preserving `StartIndex` and accepting `limit` without inventing records. Unknown and permission-revoked items return 404; cross-account requests return 403 and unmodeled `fields` projections explicitly return 501. The privileged user can retrieve an empty result for a privately indexed item, without exposing private membership information to an ordinary viewer. Rust HTTP regressions for credentials, grants, restart, revocation, and response shape passed [CI 40031984883](https://github.com/sguzman/pelagos/actions/runs/40031984883). Real BoxSet metadata import/indexing, nonempty membership lookup, broader user policies, and unmodified TV-client acceptance remain **unimplemented/unverified**.

## Initial confined original-media download transport (synthetic only)

Pinned Jellyfin v12.3 uses `GET /Items/{itemId}/Download` with `Policies.Download` (`EnableContentDownloading`) and per-item `CanDownload`. Pelagos first implemented a **restricted, deliberately non-parity** subset for administrator accounts; this was later extended by the explicit user grants described below. The transport serves authorized, indexed audio/video files are delivered through the same Linux descriptor-confined reader as direct play. It supports full responses, single byte ranges, original MIME types, `Content-Disposition: attachment` with sanitized ASCII fallback and percent-encoded UTF-8 `filename*`, and `Cache-Control: private, no-store`. Unauthenticated callers return 401; ordinary accounts lacking an explicit download permission return 403 even when granted a library; photo records and missing IDs are not offered as downloadable audio/video. A post-index symlink swap escaping the root is forbidden. [CI 40033133007](https://github.com/sguzman/pelagos/actions/runs/40033133007) passed full Rust suites, including these native HTTP and confinement fixtures. This is **not** proof of downstream Jellyfin client interoperability, full download permission policy, download auditing or production-safe operation.

## Durable independent account download permission (synthetic only)

Pelagos now models a preliminary `UserPolicy.EnableContentDownloading` value backed by an additive SQLite `pelagos_download_grants` table. Administrators retain an implicit grant; ordinary accounts default to denied even when allowed to browse or directly stream an indexed root. Local `download-grant USER` and `download-revoke USER` CLI commands change the permission without touching the original media. An already-issued token resolves the current permission at each request, and grant removal is immediate. The separate per-library authorization still applies: granting download privilege does not make otherwise ungranted library items visible or downloadable. A restarted router sees the stored grant; `/Users/Me` reflects the current policy. Native HTTP and SQLite regressions passed [CI 40033786005](https://github.com/sguzman/pelagos/actions/runs/40033786005), including deny-before-grant, positive attachment transfer, disabled and re-enabled library grants, revocation and serialized policy fields. **Full Jellyfin item-specific CanDownload policy, first-party admin UI, audit events, and real unmodified client acceptance remain unimplemented/unverified.**

## Strict byte-range request validation (synthetic only)

The shared original-file reader now accepts only one parseable HTTP `Range` field. An opaque/non-UTF-8 value, multiple `Range` headers, comma-delimited multipart request, malformed bounds, or unsatisfiable range returns HTTP 416 with the original size in `Content-Range: bytes */N`, rather than accidentally turning the request into a full-media response. Valid single-range requests still deliver 206 with exact lengths and body slices. This guard is shared by pinned `GET /Items/{itemId}/File`, native audio/video streaming, and `GET /Items/{itemId}/Download`. Native HTTP integration tests passed [CI 40034407511](https://github.com/sguzman/pelagos/actions/runs/40034407511), including opaque header bytes and duplicated fields. Rejecting syntactically invalid ranges is a conservative Pelagos policy, **not** a claim of full upstream Jellyfin multipart/If-Range behavior or unmodified-device acceptance.

## Indexed local theme-media discovery (synthetic only)

Pinned Jellyfin v12.3 `LibraryController.GetThemeSongs`, `GetThemeVideos`, `GetThemeMedia` return `ThemeMediaResult` / `AllThemeMediaResult` values with `OwnerId`, `Items`, `TotalRecordCount`, and the combined `SoundtrackSongsResult`. Pinned `Emby.Naming/Common/NamingOptions.cs` recognizes the audio filename `theme`, `theme-music` audio subdirectories, and `backdrops` video subdirectories. Pelagos now recognizes those exact local patterns in its authorized SQLite catalog, associates them with narrowly recognized owner folders or uniquely located movie files, and sets `ExtraType` for song/video response DTOs. It supports `sortBy=SortName` and `sortOrder=Ascending/Descending`. Unauthorized/missing items cannot leak theme identities; library grant revocation applies immediately, and returned IDs are directly playable via Pelagos's existing original audio/video stream transport. Unsupported sorting and `inheritFromParent=true` remain 501 rather than inventing upstream parent ownership. A combined result returns the pinned empty `SoundtrackSongsResult` default because soundtrack indexing is unimplemented. Full inherited extras, broad Jellyfin metadata-provider behavior, arbitrary audio/video extras matching, and unmodified Smart TV clients remain **unimplemented/unverified**. Native HTTP regression suites passed [CI 40037734907](https://github.com/sguzman/pelagos/actions/runs/40037734907); that does not establish full Jellyfin client acceptance.
