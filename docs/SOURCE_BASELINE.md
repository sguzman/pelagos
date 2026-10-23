# 📌 Upstream source baseline

- **Project:** [Jellyfin Server](https://github.com/jellyfin/jellyfin)
- **Release target:** [v12.3](https://github.com/jellyfin/jellyfin/releases/tag/v12.3)
- **Pinned commit:** [`06de749d435e2d0be0c5119a41f2300fdd7143e5`](https://github.com/jellyfin/jellyfin/commit/06de749d435e2d0be0c5119a41f2300fdd7143e5)
- **Reference tree:** [v12.3](https://github.com/jellyfin/jellyfin/tree/v12.3)
- **License:** GPL-2.0; do not copy or translate upstream code into the MIT repository without a licensing decision.

The target is **all built-in C# server functionality**, not just the public API. Root assemblies include Jellyfin.Api, Jellyfin.Data, Jellyfin.Server, Jellyfin.Server.Implementations, Emby.Naming, Emby.Photos, Emby.Server.Implementations, MediaBrowser.Common, MediaBrowser.Controller, MediaBrowser.LocalMetadata, MediaBrowser.MediaEncoding, MediaBrowser.Model, MediaBrowser.Providers, and MediaBrowser.XbmcMetadata. Maintain a file/class/endpoint-level inventory from this pinned tag before claiming parity.

## Baseline update policy

Pin all differential fixtures and API schemas to this release. Later upstream versions enter as explicit upgrade tracks rather than silently redefining the MVP. File/class inventory, REST routes, behavior, migrations and default configuration all need cross-references to Rust owners and parity tests. No feature is presumed compatible based on shared endpoint names alone.
