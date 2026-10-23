# 📡 Jellyfin v12.3 controller attribute inventory

**Partial inventory:** 10/60 upstream controllers, 152 HTTP method attributes. Extracted from the version-pinned C# controller files. See [machine matrix](api-contracts.json).

This is **not** a complete API inventory and **not** accepted parity. Method attribute lines alone do not establish route registration, authentication, parameter binding, response schemas, event streams or actual TV client behavior. Fields with `path: null` require manual route-attribute resolution.

- **SystemController** — `System` — 10 method attributes
- **UserController** — `Users` — 18 method attributes
- **QuickConnectController** — `QuickConnect` — 4 method attributes
- **ItemsController** — `/` — 8 method attributes
- **LibraryController** — `/` — 25 method attributes
- **MediaInfoController** — `/` — 5 method attributes
- **PlaystateController** — `/` — 14 method attributes
- **SessionController** — `/` — 16 method attributes
- **ImageController** — `/` — 45 method attributes
- **VideosController** — `Videos` — 7 method attributes

**Next:** expand to all 60 controllers, audit class-level route composition and cross-reference each HTTP contract to Rust handlers and differential tests before crediting a port.
