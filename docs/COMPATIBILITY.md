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
