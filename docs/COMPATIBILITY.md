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

**Current state:** the HTTP server exposes only a provisional public-system endpoint. It is not a passing connection, login or playback test. The actual TV apps remain the final acceptance authority.

## Discovery scaffold evidence

Rust-based UDP v12.3 auto-discovery now exists and passes loopback network tests ([CI 39954060555](https://github.com/sguzman/pelagos/actions/runs/39954060555)). It is disabled by default and can only advertise a deliberately LAN-bound HTTP listener. All actual Smart TV app discovery cells above remain **Not tested** until a real, unmodified client is observed. Client authentication and streaming are not yet implemented.

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
