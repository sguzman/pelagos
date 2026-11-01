# Pelagos contract replay and security regression tests

These checks use **isolated synthetic media and accounts**. They do not require an installed Jellyfin server, a real media library or a Smart TV, and must never use or publish private user data.

## Replay corpus

- `tests/contract_replay.rs` runs `tests/fixtures/pinned_public_replay.json` through the actual Pelagos Axum router.
- The fixture labels its source reference `jellyfin/jellyfin@06de749d435e2d0be0c5119a41f2300fdd7143e5` and covers public/private endpoint status codes, typed JSON response fields, MIME types, synthetic password login attempts, and known unknown routes.
- The only account and credentials are created by the test in a temporary state directory. They are intentionally synthetic, not real Jellyfin account records.
- **This is not a captured Jellyfin response golden corpus.** A source-derived route shape and a Pelagos response assertion do not establish full upstream payload parity, unchanged-client acceptance, or the second T011 ledger check.
- For new cases, include `name`, `method`, `path`, `auth` (`none` or `synthetic-viewer`), and expected `status`. Optional `body`, `headers`, `content_type_prefix`, `json_types` (JSON Pointer → type), and `text_exact` must remain synthetic. Unknown fixture fields fail deserialization.

## Security regression corpus

- `tests/security_harness.rs` indexes two temporary isolated libraries and verifies an authorized viewer cannot read the other library, follow a media symlink, or authenticate by passing the session token through an `ApiKey` URL parameter.
- Encoded traversal and malformed item-identifier routes must not expose the sentinel private-media content.
- A deterministic seeded corpus of malformed and valid byte-range header candidates goes through the actual authenticated HTTP transport. The test rejects server failures, unexpected response statuses, oversized bodies and cross-library content disclosure.
- This is a **deterministic mutation regression**, not a claim that native libFuzzer coverage, automated penetration testing, every authorization route, or production security validation has been completed.

## Process lifecycle and signal handling

- `tests/lifecycle_signal.rs` starts the compiled Pelagos binary as an isolated child process, binding a temporary loopback port with a fresh temporary state directory. It tests both `SIGINT` and `SIGTERM` and requires a successful exit. It never signals an existing user session, an installed service, or the desktop.
- The daemon uses Axum graceful HTTP draining on normal shutdown. After the server stops, it cancels any opt-in UDP discovery worker; the drain has a 30-second upper bound before the process reports a shutdown error rather than hanging indefinitely.
- This test establishes a narrow Linux process-lifecycle regression, not a systemd deployment, actual remote-client interruption test, long stream drain guarantee, or production operational acceptance.

## Commands

Run from the repository root using the existing Rust toolchain. Media-specific tests require the `ffmpeg` and `ffprobe` executables, as the CI workflow provides.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --test contract_replay
cargo test --test security_harness
env PELAGOS_REQUIRE_FFMPEG=1 cargo test --all-targets
cargo run --quiet --bin pelagos-roadmap -- --check
cargo run --quiet --bin pelagos-matrix -- --check
cargo run --quiet --bin pelagos-api-matrix -- --check
```

Passing these synthetic tests is useful evidence for their narrow implementation. It does not replace the pinned upstream differential fixture, real-device acceptance, performance testing or a full feature/parity audit.
