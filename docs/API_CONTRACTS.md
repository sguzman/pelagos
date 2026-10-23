# 📡 Jellyfin v12.3 controller attribute inventory

**Partial inventory:** 20/60 upstream controllers, 185 HTTP method attributes. Extracted from version-pinned C# controller files. See [machine matrix](api-contracts.json).

This is **not** a complete API inventory and **not** accepted parity. Method attribute lines alone do not establish route registration, authentication, parameter binding, response schemas, event streams or actual TV client behavior. Fields with `path: null` require manual route-attribute resolution.

- **ActivityLogController** — `System/ActivityLog` — 1 method attributes
- **ApiKeyController** — `Auth` — 3 method attributes
- **ArtistsController** — `Artists` — 3 method attributes
- **AudioController** — `Audio` — 4 method attributes
- **BackupController** — `Backup` — 4 method attributes
- **BrandingController** — `Branding` — 3 method attributes
- **ChannelsController** — `Channels` — 5 method attributes
- **ClientLogController** — `ClientLog` — 1 method attributes
- **CollectionController** — `Collections` — 3 method attributes
- **ConfigurationController** — `System` — 6 method attributes
- **ImageController** — `/` — 45 method attributes
- **ItemsController** — `/` — 8 method attributes
- **LibraryController** — `/` — 25 method attributes
- **MediaInfoController** — `/` — 5 method attributes
- **PlaystateController** — `/` — 14 method attributes
- **QuickConnectController** — `QuickConnect` — 4 method attributes
- **SessionController** — `/` — 16 method attributes
- **SystemController** — `System` — 10 method attributes
- **UserController** — `Users` — 18 method attributes
- **VideosController** — `Videos` — 7 method attributes

**Next:** expand to all 60 controllers, audit class-level route composition and cross-reference each HTTP contract to Rust handlers and differential tests before crediting a port.
