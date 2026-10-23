# 🧭 Full C# server port inventory

Reference: [Jellyfin Server v12.3](SOURCE_BASELINE.md). Status is intentionally conservative. Every upstream file, type, API controller and behavior needs an ownership mapping before declaring completeness. This is a **domain-level inventory**, not yet the exhaustive class-by-class mapping.

| C# source domain | Rust target responsibility | Status | Milestone |
| --- | --- | --- | --- |
| Jellyfin.Server / core host | Lifecycle, config, dependency wiring | ⚪ Scaffold only | M0 / M5 |
| Jellyfin.Api / controllers / WebSockets | REST contracts, events, client compatibility | 🟡 Public info stub only | M1 |
| Jellyfin.Data / MediaBrowser.Model | Core entities and DTOs | ⚪ Not ported | M1 / M2 |
| MediaBrowser.Common / Controller | Base services, auth, users, sessions | ⚪ Not ported | M1 / M5 |
| Emby.Naming | Media naming and resolvers | ⚪ Not ported | M2 |
| Emby.Photos | Image/photo metadata | ⚪ Not ported | M2 / M4 |
| Emby.Server.Implementations | Persistence, scans, server services | ⚪ Not ported | M0 / M2 |
| Jellyfin.Server.Implementations | Core hosting and server implementations | ⚪ Not ported | M0 / M5 |
| MediaBrowser.LocalMetadata / XbmcMetadata | NFO, sidecar and XML metadata | ⚪ Not ported | M2 |
| MediaBrowser.Providers | Metadata sources, artwork and merging | ⚪ Not ported | M2 |
| MediaBrowser.MediaEncoding | Probe, audio/video encoding and streaming | ⚪ Not ported | M3 |
| MediaBrowser.Controller LiveTv / SyncPlay / Subtitles / Trickplay | Advanced playback and TV features | ⚪ Not ported | M3 / M4 |
| QuickConnect / Devices / Authentication / Security | Client pairing, auth and policies | ⚪ Not ported | M1 / M5 |
| Plugins / Tasks / Backup | Extensions, scheduling, restore | ⚪ Not ported | M5 |

Mandatory follow-up: generate machine-readable **file-by-file**, **API-route-by-route**, and **behavior-by-behavior** matrices pinned to source SHA. Completeness requires coverage, matching acceptance fixtures, and no unresolved deviations. Do not mistake a list of domains for complete mapping.
