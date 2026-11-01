# Pelagos 🌊🦀 — autonomous development instructions

This is the standing contract for Codex and other delegated development agents. The user can simply say **"continue"** to authorize the next sensible, bounded iteration. Select work from the existing specification without repeatedly asking the user to choose. Do not invent features or dilute the acceptance contract to produce a completion claim.

## Scope and source of truth

- Pelagos is a **native Rust replacement for the complete Jellyfin Server v12.3**, including protocol and behavioral parity and unchanged Smart TV applications. **All milestones M0–M6 are mandatory for the first MVP.** Passing an early playback demonstration is progress, not completion.
- Read the smallest relevant section of `docs/ROADMAP.md`, `docs/progress.json`, `docs/API_CONTRACTS.md`, `docs/COMPATIBILITY.md`, `docs/SOURCE_BASELINE.md` and `docs/PROVENANCE.md`. Do not dump whole files or unrelated CI logs.
- `docs/progress.json`, `docs/port-matrix.json` and `docs/api-contracts.json` own their respective acceptance and source inventories. Generated Markdown/README progress must remain consistent with them. Maintain stable IDs and acceptance criteria; **do not rewrite, split, weaken or delete checks merely to inflate progress**.
- Preserve a clean-room boundary. Independently implement interoperable behavior in Rust; do not copy or directly translate GPL-2.0 Jellyfin code into the MIT project without first resolving licensing and attribution per `docs/PROVENANCE.md`. No credentials, personal conversation context, user media, tokens or private server data in the public repo.
- Keep the existing Jellyfin installation and users' media **untouched**. Use isolated, synthetic test data and separate Pelagos state. No system-service changes, live-device tests, migration, production rollout or destructive action without explicit authorization. No reboot or desktop-session disruption.
- Work on `main` by default. Do not create a feature branch unless requested. Never overwrite unrelated local modifications or force-push. Commit and push verified, self-contained changes when authorized. Use Rust and existing tools; do not introduce Python or npm dependencies.

## Autonomy: meaning of “continue”

1. Record the session's initial commit SHA and the initial `docs/progress.json` before modifying files; inspect `git status --short` and preserve uncommitted work. Only attribute changes since this baseline to the session.
2. Continue the current documented incomplete task or independently select the next highest-value, bounded item with clear evidence-based acceptance. Prefer dependencies and code paths useful to actual unmodified Jellyfin clients. Make sensible architectural choices within the existing contract.
3. Implement behavior **and** regression/HTTP tests. For upstream compatibility, compare against the pinned Jellyfin protocol/baseline where possible; tests of fabricated requests alone do not prove real-client parity. Record precisely what is and is not verified.
4. Run relevant quality gates. Fix failures under your control. Use real GitHub CI run IDs and device/client evidence only after obtaining them. A pending or inaccessible run is **not** a pass; a synthetic test cannot satisfy a real-device acceptance check.
5. Only mark a ledger check accepted when the evidence meets that check's *entire* wording. Regenerate/check derived documentation. If externally blocked, leave it incomplete and switch to another independent useful task when possible; summarize genuine blockers instead of guessing.
6. Complete iterations with a concise report, then stop. The next **“continue”** authorizes the next iteration. Do not create busywork after genuine 100% verified completion.

## Verification

Use the repository's current CI-equivalent gates (and any relevant targeted tests):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
PELAGOS_REQUIRE_FFMPEG=1 cargo test --all-targets
cargo run --quiet --bin pelagos-roadmap -- --check
cargo run --quiet --bin pelagos-matrix -- --check
cargo run --quiet --bin pelagos-api-matrix -- --check
```

Install/ensure `ffmpeg` and `ffprobe` for the media test suite as CI does. Distinguish local checks, GitHub Actions evidence, pinned-server comparison and real unmodified-client acceptance. Do not claim any test passed that was not run.

## Required per-iteration report

Be brief. Report **only touched milestone(s), phase(s), task(s) and related features**; do not list unchanged tasks at 100%.

- **Implemented:** the actual functionality and touched task IDs, with existing emoji labels.
- **Progress:** show a 10-block bar, numeric **total project %**, and signed **session delta in percentage points (pp)**. Show equivalent bars, % and pp deltas for *only touched* milestones/phases/tasks, from the captured initial `docs/progress.json`; do not substitute changes in code volume for evidence.
- Compute comparisons from the ledger's accepted atomic-check task percentages, not visual rounding. Report **two decimal places** for exact overall and delta where possible; do not call a relative percent change a pp delta. Use **+0.00 pp** for an iteration with no newly accepted checks.
- **Verification:** tested commands and outcomes, actual CI run/commit if confirmed, and specific compatibility evidence or its absence.
- **Delivery:** commit SHA, push status and any uncommitted changes.
- **Next:** one concrete feasible incomplete check or blocking external dependency.

Example only (illustrative figures, not measured progress):

```
🌊 Pelagos total       ████░░░░░░ 38.24% (+0.19 pp)
🔌 M1 / P4 / T123      ██████░░░░ 60.00% (+20.00 pp)
✅ Local: fmt, clippy, tests, ledger and matrix checks
🧪 Real Smart TV: not tested
📦 abc1234 — pushed to main
➡️ Next: <specific unfinished acceptance check>
```

Public documentation and commits should remain technical and professional, without personal or private conversational context. Do not artificially pursue 100%; it must be earned by meeting the complete compatibility contract.
