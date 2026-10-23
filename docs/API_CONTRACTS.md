# 📡 Jellyfin v12.3 controller attribute inventory

**Source-controller coverage:** 60/60 upstream controllers, 419 HTTP method attributes. Extracted from version-pinned C# controller files. See [machine matrix](api-contracts.json).

This is **not a fully audited or implemented API inventory** and **not** accepted parity. Method attribute lines alone do not establish route registration, authentication, parameter binding, response schemas, event streams or actual TV client behavior. Fields with `path: null` require manual route-attribute resolution. Controller source coverage is complete at the root folder level; coverage beyond this folder, plus detailed contract review, remain open.

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
- **DashboardController** — `/` — 2 method attributes
- **DevicesController** — `Devices` — 5 method attributes
- **DisplayPreferencesController** — `DisplayPreferences` — 2 method attributes
- **DynamicHlsController** — `/` — 9 method attributes
- **EnvironmentController** — `Environment` — 5 method attributes
- **FilterController** — `/` — 2 method attributes
- **GenresController** — `Genres` — 2 method attributes
- **HlsSegmentController** — `/` — 5 method attributes
- **ImageController** — `/` — 45 method attributes
- **InstantMixController** — `/` — 8 method attributes
- **ItemLookupController** — `/` — 11 method attributes
- **ItemRefreshController** — `Items` — 1 method attributes
- **ItemsController** — `/` — 8 method attributes
- **ItemUpdateController** — `/` — 3 method attributes
- **LibraryController** — `/` — 25 method attributes
- **LibraryStructureController** — `Library/VirtualFolders` — 8 method attributes
- **LiveTvController** — `LiveTv` — 40 method attributes
- **LocalizationController** — `Localization` — 4 method attributes
- **LyricsController** — `/` — 6 method attributes
- **MediaInfoController** — `/` — 5 method attributes
- **MediaSegmentsController** — `MediaSegments` — 1 method attributes
- **MoviesController** — `Movies` — 1 method attributes
- **MusicGenresController** — `MusicGenres` — 2 method attributes
- **PackageController** — `/` — 6 method attributes
- **PersonsController** — `Persons` — 2 method attributes
- **PlaylistsController** — `Playlists` — 11 method attributes
- **PlaystateController** — `/` — 14 method attributes
- **PluginsController** — `Plugins` — 9 method attributes
- **QuickConnectController** — `QuickConnect` — 4 method attributes
- **RemoteImageController** — `/` — 3 method attributes
- **ScheduledTasksController** — `ScheduledTasks` — 5 method attributes
- **SearchController** — `Search/Hints` — 1 method attributes
- **SessionController** — `/` — 16 method attributes
- **StartupController** — `Startup` — 7 method attributes
- **StudiosController** — `Studios` — 2 method attributes
- **SubtitleController** — `/` — 10 method attributes
- **SuggestionsController** — `/` — 2 method attributes
- **SyncPlayController** — `SyncPlay` — 22 method attributes
- **SystemController** — `System` — 10 method attributes
- **TimeSyncController** — `/` — 1 method attributes
- **TrailersController** — `Trailers` — 1 method attributes
- **TrickplayController** — `/` — 2 method attributes
- **TvShowsController** — `Shows` — 4 method attributes
- **UniversalAudioController** — `/` — 2 method attributes
- **UserController** — `Users` — 18 method attributes
- **UserLibraryController** — `/` — 20 method attributes
- **UserViewsController** — `/` — 4 method attributes
- **VideoAttachmentsController** — `Videos` — 1 method attributes
- **VideosController** — `Videos` — 7 method attributes
- **YearsController** — `Years` — 2 method attributes

**Next:** cross-check routes against v12.3 OpenAPI, audit class-level route composition and cross-reference HTTP contracts to Rust handlers and differential tests before crediting a port.
