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
