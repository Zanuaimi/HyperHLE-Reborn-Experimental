# CI performance plan: the macOS x86_64 job

The release workflow is gated by `build-osx (x86_64-apple-darwin)`. Every other
job finishes in 6–13 minutes; this one takes 24–39. Because
`publish-hyperhle-release` needs *all* build jobs, its duration is the whole
workflow's duration.

Phases 0, 1 and 2 of this plan are implemented in
`.github/workflows/HyperHLE_release.yml`, and so is phase 3a
(`codegen-units`) as a per-target `env:` override. Phase 3b/3c (thin LTO,
`ld64.lld`) change what is compiled and are left as a decision; §6 records
exactly what shipped. The measurement sections below are kept as the
justification for the changes.

## 1. Measurements

Job durations from `gh api .../actions/runs/<id>/jobs` (minutes):

| Job | run 40270182377 | typical |
|---|---|---|
| build-osx (x86_64) | **23.8** | 24–39 |
| build-win | 12.7 | ~13 |
| build-ios | 12.6 | ~13 |
| build-osx (aarch64) | 11.9 | ~12 |
| build-android | 6.8 | ~7 |
| build-linux | 5.7 | ~6 |
| publish-hyperhle-release | 1.7 | ~2 |

Step timings for the x86_64 job, run 40270182377 (warm) and three earlier runs:

| Step | warm | run A | run B | run C |
|---|---|---|---|---|
| Checkout | 0.72 | 1.05 | 1.17 | 0.93 |
| Cache Rust build artifacts | 0.67 | 0.57 | 0.73 | 0.47 |
| Cache Boost | 0.60 | 0.70 | 0.88 | 0.85 |
| Setup CMake | 0.15 | — | — | — |
| Cache LLVM 12 | 0.87 | 0.68 | 1.03 | 1.03 |
| common-3.0 SDK | 0.03 | — | — | — |
| **Test** (`cargo test`) | **6.93** | 9.62 | 11.13 | 8.62 |
| **Build** (`cargo build --release`) | **13.35** | 25.78 | 17.80 | 14.48 |
| .app + DMG | 0.25 | 0.42 | — | — |
| upload-artifact | 0.08 | — | — | — |
| **total** | **23.8** | **39.3** | **33.5** | **27.1** |

Two facts fall out of this immediately:

1. **`Test` is 7–11 min and produces nothing the release ships.** The aarch64
   leg of the same matrix skips it entirely (`if: matrix.target ==
   'x86_64-apple-darwin'`) and finishes in 11.9 min.
2. **`Build` ranges 13–26 min for the same commit graph.** That spread is a
   cache hit/miss difference, not intrinsic work.

Cache inventory (`gh api .../actions/caches`, 39 entries):

```
total                             10.59 GB   (GitHub limit: 10 GB)
rust-cache entries  22 entries     7.18 GB
LLVM 12             llvm_12_0_0_macOS_x64        0.74 GB
Boost *             4 entries                   0.55 GB
macOS x64 rust-cache: exactly 1 entry (v0-rust-build-osx-Darwin-x64-<lock-hash>-<toolchain>)
```

## 2. Findings, in priority order

### F1 — Tests are on the artifact critical path (7–11 min, ~29%)

`cargo test --target x86_64-apple-darwin` compiles the entire dependency graph a
second time in the **dev** profile (`cargo build --release` shares nothing with
it), builds TestApp from `tests/TestApp_source` with clang 12 + the common-3.0
SDK, and runs ~11 integration tests that each spawn the emulator. None of that
is needed to produce `HyperHLE_macOS_x86_64.dmg`, yet the DMG upload waits for
it, and so does the release.

The `Test` step also drags in the two heaviest non-Rust dependencies of the job:
the LLVM 12 cache restore (0.7–1.0 min) and the SDK download, plus `rustup
target add` — all only for TestApp.

### F2 — The repo cache is over its limit, so the x86_64 cache gets evicted (13 → 26 min)

10.59 GB of caches exist against GitHub's **10 GB per-repository limit**, which
evicts least-recently-used entries. The x86_64 rust-cache key includes a hash of
`Cargo.lock`; every dependency change creates a *new* key and the old one (0.47
GB) becomes eviction fodder while six jobs keep saving. When the macOS x64 entry
is gone, `Build` runs cold: 25.78 min instead of 13.35.

Cheap confirmation for the next run: the `Cache Rust build artifacts` step log
prints `Cache miss` / `cache-hit: false` when this happens.

The cost of a cold release build is not only wall-clock: it is also the run that
takes 39 minutes end to end.

### F3 — The release build's tail is single-threaded by configuration

`Cargo.toml` ships:

```toml
[profile.release]
opt-level = 3
codegen-units = 1
lto = "fat"
```

With `lto = "fat"` + `codegen-units = 1`, the final LTO *and* machine-code
generation happen as one unit: on the 4-vCPU Intel runner the last several
minutes of every build use one core while the other three idle. This is
deliberate (few-percent runtime win) and should stay the default for what we
ship — but CI does not have to pay full price for the last 1–2%.

### F4 — Overhead steps spend ~3.4 min before any compiling happens

`Checkout` (0.7–1.2 min) does `submodules: recursive` + `fetch-depth: 0` on all
six build jobs. The version string needs `git rev-parse HEAD`, `git status` and
`git describe --tags --abbrev=0` — that is depth 1 plus tags, not full history
(only `check-hyperhle-release` needs history, for the changelog range).

`Cache Boost` (0.6–0.9 min) is restoring a 0.14 GB archive. Boost is only
*required* on Windows and Android (`src/cpu/dynarmic_wrapper/build.rs` panics for
those targets); on macOS/Linux it is optional if `vendor/boost` exists. Keep it,
but it is the second-largest pure-overhead item after LLVM.

### F5 — The `hdiutil` retry loop is a guard, and it is left alone

The DMG step retries `hdiutil create` up to five times, backing off
5/10/15/20 s, so a failing run can add 50 s of `sleep`. In all four samples the
step took 0.25 min and never retried, and the back-off only runs once the step
has already failed, i.e. on a run that would otherwise ship no DMG. Shortening
it would save nothing measurable on a successful build while reducing the
margin on the failure it exists for, so the loop stays as it is. (This was the
one item in this plan that was tried and reverted.)

### F6 — Not a problem, for the record

`cargo run --package touchHLE_version` twice in the DMG step: the package has no
dependencies, and the whole two-invocation step costs 0.25 min including the
DMG. Ignore it.

## 3. The plan

Order is by savings-per-risk. Phases 1 and 2 change nothing about what is
compiled or shipped; phase 3 does, and needs a decision.

The YAML below is the design sketch that was implemented, not a copy of the
current workflow - edit `.github/workflows/HyperHLE_release.yml`, and see §6 for
where the shipped version differs.

### Phase 0 — measure once (10 min, do it first)

```yaml
      - name: Build
        env:
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"   # optional, see phase 3a
        run: cargo build --target ${{ matrix.target }} --release --timings
      - uses: actions/upload-artifact@main
        if: always()
        with:
          name: cargo-timings-${{ matrix.target }}
          # As shipped, both candidate locations are listed: Cargo documents
          # `<target-dir>/cargo-timings`, and a `--target` build may put it
          # under the per-target tree.
          path: |
            target/cargo-timings/
            target/${{ matrix.target }}/cargo-timings/
```

`cargo-timings/cargo-timing.html` splits the 13 min between native dependency
builds (SDL, openal-soft, dynarmic, libxml2, rusqlite), Rust codegen, LTO and
linking. Everything below is a bet on proportions; this file turns it into
arithmetic. Upload it once, then delete the `--timings` and the upload step.

### Phase 1 — take tests off the critical path (saves 7–11 min; no artifact change)

Split `build-osx` into a release-only matrix job and a separate test job, and
make publishing depend on both.

```yaml
  build-osx:
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-apple-darwin
            runner: macos-15-intel
            artifact: HyperHLE_macOS_x86_64
          - target: aarch64-apple-darwin
            runner: macos-15
            artifact: HyperHLE_macOS_AArch64
    runs-on: ${{ matrix.runner }}
    steps:
      - uses: actions/checkout@main
        with:
          submodules: recursive
          fetch-depth: 1          # F4; keep fetch-tags for the version string
          fetch-tags: true
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}
          save-if: ${{ github.ref == 'refs/heads/trunk' }}   # F2
      - uses: lukka/get-cmake@latest
        with:
          cmakeVersion: "~3.29.0"
      - name: Cache Boost
        id: cache-boost
        uses: actions/cache@main
        with:
          path: vendor/boost
          key: boost_1_81_0_macos
      # ... download/extract boost, unchanged ...
      - run: rustup target add ${{ matrix.target }}
      - name: Build
        run: cargo build --target ${{ matrix.target }} --release
      - name: Create .app bundle and DMG      # unchanged
      - uses: actions/upload-artifact@main    # unchanged

  test-macos:
    runs-on: macos-15-intel
    # TestApp needs its own toolchain: LLVM 12 and the common-3.0 SDK.
    steps:
      - uses: actions/checkout@main
        with:
          submodules: recursive
          fetch-depth: 1
          fetch-tags: true
      - uses: Swatinem/rust-cache@v2
        with:
          key: test-x86_64-apple-darwin
          save-if: ${{ github.ref == 'refs/heads/trunk' }}
      - uses: lukka/get-cmake@latest
        with:
          cmakeVersion: "~3.29.0"
      - name: Cache Boost
        id: cache-boost
        uses: actions/cache@main
        with:
          path: vendor/boost
          key: boost_1_81_0_macos
      # ... download/extract boost, unchanged ...
      - name: Cache LLVM 12
        id: cache-llvm
        uses: actions/cache@main
        with:
          path: tests/llvm
          key: llvm_12_0_0_macOS_x64
      # ... download/extract LLVM, unchanged ...
      - name: Get copy of common-3.0-sdk
        run: cd tests && curl -L https://github.com/touchHLE/common-3.0-sdk/releases/latest/download/common-3.0.sdk-macOS-universal.tar.gz | tar xzv
      - run: rustup target add x86_64-apple-darwin
      - name: Test
        run: cargo test --target x86_64-apple-darwin
```

And in `publish-hyperhle-release`: add `test-macos` to `needs` and to the `if:`.

The `key:` lines in this sketch were **not** applied - see §6, under phase 2.

Tests still gate the release — they just no longer hold the DMG hostage. Their
wall-clock (~10–13 min) is already within the range of the Windows and iOS jobs,
so they add nothing to the critical path.

### Phase 2 — stop the cache eviction (removes the 26–39 min outliers)

1. `save-if: ${{ github.ref == 'refs/heads/trunk' }}` on every `rust-cache` use
   (above). PR runs restore but do not save, so six jobs stop multiplying
   entries per lockfile hash.
2. Lower the cache footprint: `cache-targets: "true"` is wanted, but **delete the
   stale entries** once (`gh api -X DELETE
   repos/Zanuaimi/HyperHLE-Reborn-Experimental/actions/caches/<id>`) and let
   key-stability keep them alive afterwards.
3. Pin the action: `Swatinem/rust-cache@v2` instead of `@master`, so a silent
   upstream change cannot alter the key layout mid-release-week.
4. Optional: run the test job's rust-cache with `cache-all-crates: false` (the
   default) — do *not* enable it; the dev-profile graph is the single largest
   cache entry producer in this repo.

After this, the `Build` step should be 13–15 min every run instead of
occasionally 26.

### Phase 3 — reduce the build step itself (needs your decision)

Three independent knobs, in order of expected gain per unit of risk:

**3a. Parallel codegen under fat LTO (recommended).**

```yaml
      - name: Build
        env:
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"
        run: cargo build --target ${{ matrix.target }} --release
```

`lto = "fat"` merges the crates, runs the LTO pass, then splits the result into
`codegen-units` partitions. With `codegen-units = 1` that split is one unit and
the 4-vCPU runner uses one core for the tail; 16 restores parallelism at the
cost of a fraction of a percent of runtime. Expected 13.35 → ~10 min, cold 26 →
~18. This is an env override, so the repo's shipped profile and every local
build stay exactly as they are — but the *artifact* is built slightly
differently, so benchmark one game before and after (the emulator's own
`--print-fps` on a fixed scene is enough).

**3b. `lto = "thin"` for x86_64 only (bigger win, real tradeoff).**

```yaml
        env:
          CARGO_PROFILE_RELEASE_LTO: ${{ matrix.target == 'x86_64-apple-darwin' && 'thin' || 'fat' }}
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"
```

Typically another 20–35% off the build, and roughly 1–3% slower emulation on
Intel Macs only (Apple Silicon keeps fat LTO). The comment in `Cargo.toml`
argues for fat LTO explicitly, so this contradicts a deliberate decision: take
it only with a measured before/after number.

**3c. Faster linker (only if Phase 0 shows linking is a big slice).**

Xcode's `ld64` is slow on large LTO outputs; `ld64.lld` from the Rust toolchain
is usually faster. Try on one run:

```yaml
        env:
          RUSTFLAGS: "-C link-arg=-fuse-ld=lld"
```

If the build fails to find `ld64.lld`, skip it — not worth adding a Homebrew
install step to save a minute.

### Phase 4 — optional, bigger, riskier

- **sccache** (`mozilla-actions/sccache-action`) with the GHA backend: gives
  per-rustc-invocation hits that survive `target/` eviction and lockfile
  changes. Helps most exactly when F2 bites. Adds a wrapper to every invocation
  and its own cache accounting.
- **Cross-build the x86_64 artifact on the arm64 runner.** An M1 core is much
  faster than the Intel runner's cores. Native dependencies (SDL, openal-soft,
  dynarmic, libxml2) would need `CMAKE_OSX_ARCHITECTURES=x86_64` plumbed through
  every `cmake`-based build script, and TestApp could not run there, so this
  moves the build but not the tests. High effort, worth it only if x86_64 remains
  the bottleneck after Phases 1–3.
- **Larger runners** (paid): 6-core macOS Intel. Costs money per minute but
  roughly halves the release build; the cheapest fix if CI minutes are
  cheaper than your time.

## 4. Expected result

| Stage | x86_64 build job | workflow total (publish included) |
|---|---|---|
| today, warm | 23.8 | ~25.5 |
| today, cold cache | 39.3 | ~41 |
| **Phase 1** | ~16 | **~18** |
| Phase 1 + 2 | ~16, stable | ~18, no 40-min outliers |
| Phase 1 + 2 + 3a | ~12–13 | **~14–15** |
| Phase 1 + 2 + 3a + 3b | ~9–10 | ~14 (bottleneck moved) |

**Know where to stop.** After ~12–13 min, the x86_64 job stops being the
critical path and the workflow is bounded by `build-win` (12.7) and `build-ios`
(12.6) plus the ~1.7 min publish job. Further macOS tuning then buys nothing
until those move. Target: x86_64 ≤ 12.6 min, not zero.

## 5. Risk and rollback

| Change | Risk | Rollback |
|---|---|---|
| Phase 1 split | Publishing could start without tests if the `needs`/`if` edit is incomplete | Add `test-macos` to `needs` + `if` in the same commit; verify the release job's graph in the Actions UI |
| Phase 1 split | Test job cache is new → first run builds dev profile cold (~13 min, off critical path) | none needed |
| `fetch-depth: 1` | `git describe` warnings if a tag is unreachable; version string unaffected (it comes from `CARGO_PKG_VERSION`) | revert to `fetch-depth: 0` |
| `save-if: trunk` | PR runs lose their own incremental cache (they still restore trunk's) | remove the input |
| 3a `codegen-units=16` | Shipped binary differs slightly in speed | delete the `env:` block |
| 3b thin LTO | Up to a few percent slower on Intel | per-target expression |
| 3c lld | Link failures on a toolchain change | delete `RUSTFLAGS` |

Nothing in Phases 1–2 touches the emulator, `Cargo.toml` or any artifact
content, so a regression there can only be a workflow-level bug.

## 6. Implementation status

Applied in `.github/workflows/HyperHLE_release.yml` and
`.github/workflows/cleanup-caches.yml`:

- **Phase 3a** — `CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"` on the Intel
  macOS build only (`"1"` for the arm64 leg, which is `Cargo.toml`'s value; the
  value must never be empty, that is a profile parse error rather than "use the
  default"). See the note on parity below.
- **F7** — `CMAKE_BUILD_PARALLEL_LEVEL: "2"` on `build-osx`, `test-macos`,
  `build-win` and `build-ios`. Expected gain differs per job: on Windows the
  `cmake` crate uses the Visual Studio generator and `--build` forwards the
  value to MSBuild as `/m`; on iOS, `platform/ios/scripts/build-sdl-shared.sh`
  already passes `-G Ninja` (parallel by default), so only the Rust-side
  wrappers (OpenAL Soft, libxml2, Dynarmic) gain there.
- **Phase 0** — new `ci_timings` `workflow_dispatch` input, default `false`.
  When set, the macOS build runs with `--timings` and uploads
  `cargo-timings-<target>` artifacts (`if: always()`, `if-no-files-found:
  ignore`). Normal runs are unaffected.
- **Phase 1** — `build-osx` no longer runs `cargo test` and no longer installs
  LLVM 12 / the common-3.0 SDK; the new `test-macos` job does that, and
  `publish-hyperhle-release` now has it in both `needs` and `if`, so a failing
  test still blocks a release.
- **Phase 2** — every `Swatinem/rust-cache` use is pinned to `@v2` and passes
  `save-if: ${{ github.ref == 'refs/heads/trunk' }}`, so pull-request runs
  restore but never store. No `key:` was added on purpose: rust-cache already
  derives its key from the job name (`v0-rust-build-osx-Darwin-x64-…` in the
  existing cache inventory), and an explicit key would have discarded the warm
  entries and forced one cold ~26 minute build.
- **F4** — `fetch-depth: 1` on the build jobs (`fetch-tags: true` is kept; only
  `check-hyperhle-release` and `publish` need history). This is safe because
  the version string only uses `rev-parse` and `status`; `describe --tags` feeds
  a warning that `src/version/build.rs` skips when the command fails.
- **F5** — *not applied.* The `hdiutil` back-off was briefly shortened to
  2/4/6/8 s, then reverted to the existing 5/10/15/20 s: it only runs after the
  step has already failed, so there was no gain to measure and one less retry
  margin on the failure it exists for.

Not applied, deliberately: phases 3b and 3c (thin LTO, lld), because they
change the shipped binary and need a measured before/after.

**Parity note for 3a.** Setting `codegen-units` through the environment means
CI's Intel artifact is tuned differently from the arm64 artifact, from the
Windows/Linux/Android artifacts, and from any local `cargo build --release`. If
the benchmark is neutral, the tidier end state is to move the setting into
`Cargo.toml` (`codegen-units = 16` alongside `lto = "fat"`) so every build
matches, and then drop the per-target expression from the workflow.

Two notes for whoever picks up the rest:

- `test-macos` has a new job name, so its rust-cache starts empty: its first
  run compiles the dev profile cold (~13 min). That is *off* the critical path.
- On the first run, check the `Cache Rust build artifacts` step in
  `build-osx (x86_64)`. If it says `Cache miss`, the eviction in F2 was indeed
  what produced the 26–39 minute runs, and phase 2 plus a one-off cleanup of
  stale entries (`gh api -X DELETE repos/<owner>/<repo>/actions/caches/<id>`,
  currently 10.59 GB against a 10 GB budget) fixes it.


## 7. Round 2: what is left after phases 1-2

With tests off the critical path, the x86_64 job is essentially one step:
`Build`. So every remaining second is either "compile less" or "compile the
same thing faster". Two things were found that cost time without any
trade-off, and both are now applied.

### F7 — the vendored CMake trees were compiled serially (**applied**)

`src/cpu/dynarmic_wrapper/build.rs`, `src/audio/openal_soft_wrapper/build.rs`
and `src/frameworks/libxml2_wrapper/build.rs` all call `cmake::Config::build()`
with no `build_arg`, and no generator is selected, so CMake uses its platform
default ("Unix Makefiles"). `cmake --build` without `--parallel` runs `make`
with no `-j`: **one source file at a time** for SDL2, OpenAL Soft, libxml2 and
Dynarmic, on a 4-core runner.

CMake's `--build` honours `CMAKE_BUILD_PARALLEL_LEVEL`, so this needs no build
script change:

```yaml
    env:
      CMAKE_BUILD_PARALLEL_LEVEL: "2"
```

Applied to `build-osx`, `test-macos`, `build-win` and `build-ios`. It only
helps when those trees are actually compiled, i.e. on a cold cache - which,
given F2, is frequent, and is exactly where 26-minute runs came from. The gain
differs per job: on Windows the `cmake` crate picks the Visual Studio generator
and `cmake --build` forwards the value to MSBuild as `/m` (a full win); on iOS,
`platform/ios/scripts/build-sdl-shared.sh` already passes `-G Ninja`, so only
the Rust-side wrappers (OpenAL Soft, libxml2, Dynarmic) gain there. Start at
`2` because cargo may run several build scripts concurrently; `4` is worth a
try once the timings are in.

### F8 — nothing reclaims the cache budget (**applied**)

The 10 GB budget is enforced by LRU eviction, and the repository simply kept
growing until the macOS x86_64 entry was the thing evicted. Manual deletion is
not a fix. `.github/workflows/cleanup-caches.yml` now runs weekly (and on
demand) and deletes entries that are older than 21 days or that push total
usage above 8 GB, oldest-used first, and writes a summary of what went. It
uses only `actions: write`, and cache entries are pure accelerators, so it
cannot break a build.

### F9 — possibly a second whole-program link (measure first)

The package declares `crate-type = ["cdylib", "rlib"]` for the iOS host. The
macOS release job only ships the executable, so if `cargo build --release`
also links `libtouchHLE.dylib`, that is a second full fat-LTO link of the
entire crate for an artefact nobody uploads. `cargo build --timings` (the
`ci_timings` input) will show whether a separate cdylib unit exists; if it
does, `cargo build --release --bin touchHLE --target ...` should drop it.
Worth checking before changing the command, because whether cargo emits the
cdylib for a `--bin` build is not obvious from the manifests.

### F10 — the remaining knobs, with their real trade-offs

| Knob | Effect | Cost |
|---|---|---|
| `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` | restores parallel codegen at the LTO tail (currently single-core) | a fraction of a percent of runtime |
| `CARGO_PROFILE_RELEASE_LTO=thin` (x86_64 only) | another 20-35% off `Build` | 1-3% slower emulation on Intel Macs |
| `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` | 20-30% off codegen | a few percent slower, everywhere |
| `Swatinem/rust-cache` `cache-targets: "false"` on the fast jobs | frees ~1.5 GB of budget | cold rebuilds for jobs that currently take 6 minutes |

Do the first one first: it is one `env:` line, it is the only one whose cost is
negligible, and it attacks the biggest remaining block.

### F11 — sccache: not with the GHA cache backend

`sccache` (via `mozilla-actions/sccache-action`) would cache individual `rustc`
and `cc` invocations across runs and survive `target/` eviction, which is
attractive given F2. But its GHA backend stores into **the same 10 GB budget**,
competing with the builds it is meant to accelerate. Only worth it with an
external backend (S3/GCS), which needs credentials. Not recommended here.

### F12 — cross-building x86_64 on the Apple Silicon runner

The arm64 leg does the same work in 10.2 minutes on a 3-core M1 while the Intel
leg needs 13.4 on 4 cores, so an M1 core is roughly 1.7x faster here. Building
`x86_64-apple-darwin` on `macos-15` (`rustup target add` + `--target`) would
move the work to the faster cores, but every `cmake`-based wrapper needs
`CMAKE_OSX_ARCHITECTURES=x86_64` plumbed through (SDL2, OpenAL Soft, libxml2,
Dynarmic) or the link fails with "building for macOS-x86_64 but attempting to
link in object file built for macOS-arm64". TestApp cannot run there, which is
fine now that tests are a separate job. Estimated 13.4 -> 9-11 minutes warm;
worth it only after F10, and it is the largest diff of any option here.

### F13 - the bottleneck moves

After F10, x86_64 lands around 11-13 minutes and stops being the critical path:
`build-win` (12m42s) and `build-ios` (12m38s) take over, plus the 1.7 minute
publish job. Further macOS work then buys nothing. Those two jobs also build SDL and
Dynarmic, and they now get `CMAKE_BUILD_PARALLEL_LEVEL` as well (§6); what they
have *not* been given is the `codegen-units` override, and no
`--timings`-driven measurement of either job has been done.

### Micro-wins not worth a PR on their own

- `cargo build --locked`: skips registry/index resolution. Safe only if
  `Cargo.lock` is always in sync; it fails the build rather than updating.
- The DMG step runs `cargo run -p touchHLE_version` twice, compiling that crate
  in the **dev** profile (a second profile for a 4-file crate, ~10-20 s total).
  Building `-p touchHLE_version` in release alongside the main build and calling
  `target/<triple>/release/touchHLE_version_bin` removes it.
- `timeout-minutes:` on every job, so a hung build cannot occupy a runner.
- `hdiutil create -format ULFO`: faster (lzfse) than the default UDZO, at the
  cost of a larger DMG. Only matters if F5-like retries appear.

## 8. Checklist

- [x] Phase 0: `ci_timings` input + `--timings` upload (run once with it on,
      read `cargo-timing.html`, note the LTO/link/native-deps split).
- [x] Phase 1: split `build-osx` / `test-macos`, add `test-macos` to the publish
      job's `needs` and `if`.
- [x] Phase 2: `save-if` on all five rust-cache uses; pin `@v2`.
- [ ] Phase 2 follow-up: delete stale cache entries (>10 GB budget).
- [x] Phase 3a: `CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"` for the Intel
      build, `"1"` for arm64.
- [ ] Phase 3a follow-up: compare `--print-fps` before/after on one heavy game;
      if neutral, move the setting into `Cargo.toml` so every build matches.
- [x] F7: `CMAKE_BUILD_PARALLEL_LEVEL` on the macOS jobs (cold-cache native
      builds were serial `make`), and on `build-win` / `build-ios`.
- [x] F5: measured the `hdiutil` loop (0.25 min, never retried); deliberately
      left unchanged.
- [x] F8: scheduled cache pruning (`.github/workflows/cleanup-caches.yml`),
      21-day / 8 GB budget.
- [ ] F9: read `cargo --timings`; if a cdylib link unit exists, try
      `--bin touchHLE`.
- [ ] Re-measure the job table; decide whether 3b/3c or F12 are still worth it,
      or whether the next target is `build-win` / `build-ios`.
- [ ] None of this has been executed yet. Every number here comes from earlier
      `trunk` runs; the workflow edits in this branch (Phases 1, 2, 3a, F4, F7,
      F8) have not had a single CI run, so treat the §4 projections as
      projections until one green run confirms them.
