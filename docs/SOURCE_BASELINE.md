# 📌 Upstream source baseline

- **Project:** [Jellyfin Server](https://github.com/jellyfin/jellyfin)
- **Release target:** [v12.3](https://github.com/jellyfin/jellyfin/releases/tag/v12.3)
- **Pinned commit:** [`06de749d435e2d0be0c5119a41f2300fdd7143e5`](https://github.com/jellyfin/jellyfin/commit/06de749d435e2d0be0c5119a41f2300fdd7143e5)
- **Reference tree:** [v12.3](https://github.com/jellyfin/jellyfin/tree/v12.3)
- **License:** GPL-2.0; do not copy or translate upstream code into the MIT repository without a licensing decision.

The target is **all built-in C# server functionality**, not just the public API. Root assemblies include Jellyfin.Api, Jellyfin.Data, Jellyfin.Server, Jellyfin.Server.Implementations, Emby.Naming, Emby.Photos, Emby.Server.Implementations, MediaBrowser.Common, MediaBrowser.Controller, MediaBrowser.LocalMetadata, MediaBrowser.MediaEncoding, MediaBrowser.Model, MediaBrowser.Providers, and MediaBrowser.XbmcMetadata. Maintain a file/class/endpoint-level inventory from this pinned tag before claiming parity.

## Baseline update policy

Pin all differential fixtures and API schemas to this release. Later upstream versions enter as explicit upgrade tracks rather than silently redefining the MVP. File/class inventory, REST routes, behavior, migrations and default configuration all need cross-references to Rust owners and parity tests. No feature is presumed compatible based on shared endpoint names alone.

## Exhaustive C# file inventory

GitHub v12.3 tree `5d01346d1cc857fc7c3c4d581205eea95dda3c0d` has 2247 C# files, including 1932 server-source files and 315 test/fuzz reference files. The complete pinned path+blob-SHA list is maintained in [`port-matrix.json`](port-matrix.json). All 1932 server candidates require ownership triage, behavior coverage and evidence before asserting full C# replacement.
