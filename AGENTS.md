# AGENTS.md

## Rules for emulator changes

These rules come from bugs that reached trunk. They apply to every agent and environment.

1. **Run the full test suite.** Run `cargo test` with `test_app` before you open a PR. TestApp needs LLVM 12 in `tests/llvm` and the common-3.0 SDK in `tests/common-3.0.sdk`; the macOS job in `.github/workflows/HyperHLE_release.yml` shows where to get them. If you cannot run TestApp, write "TestApp not run" in the PR. Never delete, skip or weaken a failing test or CI step to get a green run: fix the cause, or leave it failing and report it.
2. **Do not change shared behavior for one game.** A change that affects every app must match real iOS. If one game needs different behavior, add a general option in `src/options.rs` and enable it for that bundle ID in `touchHLE_default_options.txt`. Do not add new `bundle_identifier()` checks in code.
3. **When a game crashes, find and fix the root cause first.**
   - Investigate the last emulated API calls, loader, CPU, memory, audio or graphics state against Apple's documentation before considering any fallback.
   - If the emulator is wrong, fix the underlying API or subsystem and add a regression test (`TestApp` where possible, otherwise a Rust unit test).
   - Keep `--headless` (and `TestApp`) strict: headless runs must never mask guest `exit`, `abort`, uncaught exceptions or fatal signals, so automated tests always surface regressions immediately.
   - GUI compatibility fallbacks (such as surviving legacy DRM/anti-tamper `exit()`/`abort()` checks or ignoring unhandled `SIGPIPE` from old ad SDKs) must remain scoped to non-headless sessions and must never replace fixing an actual HLE bug.
4. **Match iOS and cite the source.** Before you implement or change an API, check Apple's documentation (old references are on web.archive.org) and touchHLE. Link the source in the commit message or the test.
5. **Search before adding.** Before you add a class, method, function or constant, search for an existing one. The `no_duplicate_*` tests fail on duplicates.

## Cursor Cloud specific instructions

### Overview

This repo contains two products:
1. **touchHLE emulator** (Rust) — an HLE iPhone OS app emulator
2. **AppDB** (Python/FastAPI) — a compatibility report web app at `appdb/`

### Building the Rust emulator

- **Git submodules must be initialized** before building: `git submodule update --init`
- The build requires `libboost-dev`, `libstdc++-14-dev`, `libsndio-dev`, and X11 development headers (`libxext-dev`, `libx11-dev`, etc.) on Linux.
- On this Cloud Agent VM (x86_64 Linux with clang), you **must** set `RUSTFLAGS="-C link-arg=-latomic"` for linking to succeed. OpenAL Soft's static build uses `std::atomic` operations that require `libatomic`, but the build scripts don't link it automatically.
- Build: `RUSTFLAGS="-C link-arg=-latomic" cargo build`
- The emulator binary requires a display/window system (SDL2) to run graphically. In headless environments, use `--help`, `--copyright`, or `--info` flags to verify the binary works.

### Linting and testing

- Lint: `dev-scripts/lint.sh` (runs comment-length check + `cargo clippy -- --deny warnings` + `cargo doc`). Note: the `t` branch has pre-existing long comment lines and clippy warnings with Rust 1.95+, so the full lint script will fail. Running `cargo clippy` alone (without `--deny warnings`) confirms no hard errors.
- Tests: `RUSTFLAGS="-C link-arg=-latomic" cargo test -- --skip test_app` (the `test_app` integration tests require a custom LLVM/SDK toolchain not available in the Cloud VM). This is not a full check: say in the PR that TestApp was not run. CI runs it on macOS.
- Format: `dev-scripts/format.sh` (requires `clang-format` for C/C++ files).

### Running the AppDB

- Setup: `cd appdb && python3 -m venv .venv && source .venv/bin/activate && pip install -e .`
- Run: `cd appdb && source .venv/bin/activate && uvicorn app.main:app --host 0.0.0.0 --port 8000 --reload`
- The SQLite DB is auto-created and seeded with demo data on first startup.
- Submitting reports requires GitHub OAuth (`GITHUB_OAUTH_CLIENT_ID` / `GITHUB_OAUTH_CLIENT_SECRET` env vars); without these, the app works in read-only mode.
- The triage JSON API requires `APPDB_TRIAGE_TOKEN` env var.

## Session notes

There is a dated handoff for the in-flight CI/graphics work in `HANDOFF.md`
(Minigore render investigation, release-workflow measurements, environment
gotchas). It is session context rather than project documentation - update or
delete it once it is stale.
