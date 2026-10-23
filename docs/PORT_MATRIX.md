# 🧭 Jellyfin C# source port inventory

**Baseline:** [Jellyfin Server v12.3](SOURCE_BASELINE.md) at pinned Git tree `5d01346d1cc857fc7c3c4d581205eea95dda3c0d`. Generated from the complete Git tree (GitHub returned `truncated=false`).

**Authoritative file list:** [`port-matrix.json`](port-matrix.json) contains all **2247** C# paths and upstream blob SHAs, their roles, statuses, and Rust replacement references. **1932** are production source candidates; **315** are upstream tests/fuzz references, not additional application modules.

**Strict accepted port coverage: `0 / 1932 = 0%`.** Two production files have *partial* Rust analogs, which **do not count as ported**. No file is marked completed. Mapping a source file does not prove behavioral parity.

| Root module | C# server sources | Unported | Partial | Accepted |
| --- | ---: | ---: | ---: | ---: |
| `Emby.Naming` | 48 | 48 | 0 | 0 |
| `Emby.Photos` | 2 | 2 | 0 | 0 |
| `Emby.Server.Implementations` | 156 | 156 | 0 | 0 |
| `Jellyfin.Api` | 169 | 168 | 1 | 0 |
| `Jellyfin.Data` | 26 | 26 | 0 | 0 |
| `Jellyfin.Server` | 89 | 89 | 0 | 0 |
| `Jellyfin.Server.Implementations` | 67 | 67 | 0 | 0 |
| `MediaBrowser.Common` | 41 | 41 | 0 | 0 |
| `MediaBrowser.Controller` | 387 | 387 | 0 | 0 |
| `MediaBrowser.LocalMetadata` | 15 | 15 | 0 | 0 |
| `MediaBrowser.MediaEncoding` | 27 | 27 | 0 | 0 |
| `MediaBrowser.Model` | 307 | 306 | 1 | 0 |
| `MediaBrowser.Providers` | 159 | 159 | 0 | 0 |
| `MediaBrowser.XbmcMetadata` | 28 | 28 | 0 | 0 |
| `SharedVersion.cs` | 1 | 1 | 0 | 0 |
| `src` | 410 | 410 | 0 | 0 |

## Acceptance process

1. Inventory every upstream source file and record its SHA and owning Rust replacement(s).
2. Derive routes, DTOs, states and side effects from the pinned version into behavioral/API contract matrices.
3. Implement independently in Rust, preserving GPL/MI​​T licensing boundaries ([provenance](PROVENANCE.md)).
4. Require passing Rust unit, integration and client/differential tests before upgrading any source status to accepted.
5. Resolve upstream file additions/removals explicitly when changing the baseline tag. A v12.3 acceptance gate may not silently float to a different upstream version.

Reference tests are useful for differential behavior but not proof of production port coverage. The source matrix is one axis of acceptance; **unmodified Jellyfin Smart TV playback remains a separate mandatory gate** ([matrix](COMPATIBILITY.md)).
