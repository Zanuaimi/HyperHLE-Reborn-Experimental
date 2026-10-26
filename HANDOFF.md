# Session handoff — 2026-10-26

Notes for whoever picks this up next. This file is session context, not project
documentation: delete or rewrite it once it is stale.

Read alongside `AGENTS.md` (rules for emulator changes) and
`dev-scripts/ci-performance-macos-x86_64.md` (the measurement/plan document for
the CI work, which is kept as living documentation rather than session notes).

---

## 1. Where things stand

Two threads are closed, one is open.

| Thread | State |
|---|---|
| EAGL presentation-route logging | **Merged** (PR #1, merge commit `bee1ffc`) |
| Release-workflow optimisation (macOS x86_64 critical path) | **Merged** (same PR) |
| Minigore x-ray sprites / black shaders | **Open, root cause not proven** |

`trunk` is at `0aa2646` ("Update .gitignore", 2026-10-26T10:10Z), one commit
after the merge. That commit's CI run published **release `v1.0.11`**, so the
work in §2 is already in a shipped build.

The fork's parent is `KlugKlugTG/HyperHLE-Reborn`; upstream `trunk` was
`3f7325d` (= tag `v1.2.10`, 2026-10-25) when the comparisons below were made.

The open investigation is worth reading first (§3) because it is the only thing
that still needs work.

### Checkout quirks you will hit immediately

- **This clone is shallow**: `.git/shallow` contains `0aa2646`, so `git log`
  shows a single commit. Run `git fetch --unshallow origin` before any history
  archaeology.
- **There is no `upstream` remote**, only `origin`
  (`Zanuaimi/HyperHLE-Reborn-Experimental`). To compare against the parent repo:
  `git remote add upstream https://github.com/KlugKlugTG/HyperHLE-Reborn.git`.
- Work on the session branch only (`arena/e77da1e8-hyperhle-reborn-experimental`);
  do not push elsewhere.
- The merged history deliberately contains a temp commit and its revert
  (`6d002ed` "roll the bundled ANGLE back … [do not merge]" → `ec989ed`
  "Revert …"). Content on `trunk` is correct (ANGLE binaries are the inherited
  2026-10 ones); the pair is only noise in `git log`.

---

## 2. What shipped in PR #1 and is already in the tree

### 2.1 EAGL presenter diagnostics — `src/frameworks/opengles/eagl.rs`

`presentRenderbuffer:` chose between the GPU-copy presenter and the
`glReadPixels` + Core Animation route inline, and only ever reported the
negative case ("no fullscreen layer found"). A bug report therefore carried no
record of how frames reached the screen.

The policy was extracted **unchanged** into a pure function plus a small amount
of logging around it:

- `PresentRoute` (`Readback` | `Direct`), `PresentDecision { route, reason }`,
  `PresentBackend { is_translator, is_native_es1, is_es2, direct_present_broken }`,
  `select_present_route(present_mode, backend)`.
- `note_present_route()` + `LAST_PRESENT_ROUTE: AtomicU8` — prints *one* line
  per mid-session route change (e.g. the black-frame probe giving up on the GPU
  presenter), and stays silent on the first frame because the one-shot line
  covers that.
- One one-shot line names route, reason, `--present-mode`, `force_composition`
  and the backend flags.
- The RAM-readback slow path now says whether it was reached because
  `--force-composition` was requested or because the drawable is not an opaque
  full-screen layer. Those two cases were previously indistinguishable, and the
  second one is a genuine bug rather than a workaround.
- Unit tests in `mod present_route_tests` (no GL context needed): the `auto`
  policy per backend, that explicit `--present-mode` values are honoured
  (including readback being unavailable on a real ES 2.0 driver), and that every
  `reason` is non-empty, single-line and ≤ 80 chars.

Log lines to grep for (all `log!`, so they always appear; the per-frame ones are
`log_dbg!` and effectively silent because `ENABLED_MODULES` is empty):

```
EAGL presenter: fullscreen layer … will be presented via … (present_mode=…, force_composition=…, translator=…, native_es1=…, es2=…): <reason> [this log will only be shown once]
EAGL presenter: layer … presentation changed from … to …: <reason> [present_mode=…, probe_says_direct_is_black=…]
EAGL presenter: no fullscreen layer found (composition is forced by --force-composition | the drawable does not cover the screen as an opaque layer); presenting renderbuffer … to layer … via RAM readback + Core Animation compositing. [this log will only be shown once]
Force Composition is enabled; bypassing the fullscreen EAGL fast path. [this log will only be shown once]
```

**There is no behaviour change, and no Force Composition change.**
`src/options.rs` and `OPTIONS_HELP.txt` were left byte-identical to `trunk`.
`--force-composition` already works by making `find_fullscreen_eagl_layer()`
return `nil` (`src/frameworks/core_animation/ca_eagl_layer.rs`), which skips the
fast-path branch entirely and falls into the readback + composite slow path.

Consequence worth remembering when reading logs: **with Force Composition on, the
one-shot "fullscreen layer … will be presented via" line never appears**, because
that branch is never reached. The slow-path line is the one to look for. Do not
"fix" that by trying to plumb force-composition into `select_present_route` — an
earlier attempt at exactly that was scrapped because the premise (that
`--force-composition` was inert) was wrong.

The `--present-mode=readback is not available on an OpenGL ES 2.0 backend`
warning was folded into the decision's `reason`, so it still appears, on the
one-shot line, instead of being tracked in two places.

### 2.2 Release workflow — `.github/workflows/HyperHLE_release.yml`

Measured baseline: `build-osx (x86_64)` 23.8 min warm / 39.3 min cold, while
`build-win` was 12.7 and `build-ios` 12.6 — the workflow duration *is* that job
because `publish-hyperhle-release` needs all builds.

Applied:

- **`test-macos` job**: `cargo test` + the LLVM 12 / common-3.0 SDK toolchain
  moved out of `build-osx`. `publish-hyperhle-release` lists it in **both**
  `needs` and `if`, so failing tests still block a release — they just no longer
  sit between the build and the DMG upload.
- **Cache churn**: every `Swatinem/rust-cache` use pinned to `@v2` with
  `save-if: ${{ github.ref == 'refs/heads/trunk' }}`, so PR runs restore but do
  not store. Deliberately **no explicit `key:`** — rust-cache already derives a
  per-job key, and adding one would discard the warm entries.
- **`CMAKE_BUILD_PARALLEL_LEVEL: "2"`** on `build-osx`, `test-macos`,
  `build-win`, `build-ios`. The `cmake` crate calls `cmake --build` without
  `--parallel` and without a generator, so the vendored SDL2 / OpenAL Soft /
  libxml2 / Dynarmic trees were built one file at a time. Full win on Windows
  (VS generator → `--build` forwards it to MSBuild as `/m`); partial on iOS
  because `platform/ios/scripts/build-sdl-shared.sh` already passes `-G Ninja`.
- **`CARGO_PROFILE_RELEASE_CODEGEN_UNITS`**: `16` for `x86_64-apple-darwin`,
  `1` for arm64. `Cargo.toml` keeps `codegen-units = 1` + `lto = "fat"`; this is
  an env override for CI only. The value must never be empty (an empty profile
  env value is a parse error, not "use the default").
- **`ci_timings` `workflow_dispatch` input** (default `false`): runs the macOS
  build with `--timings` and uploads `cargo-timings-<target>`.
- **`fetch-depth: 1`** (tags kept). `src/version/build.rs` only needs
  `rev-parse HEAD` and `status`; `describe --tags` feeds a warning that is
  skipped when the command fails.
- **`.github/workflows/cleanup-caches.yml`**: weekly (Mon 03:17 UTC) and manual
  pruner, 8 GB budget / 21 days, oldest-used first, `actions: write` only.

Deliberately unchanged, with reasons recorded in the plan doc: the `hdiutil`
retry back-off (it only runs after the step has already failed), the
non-cancelling `trunk` concurrency group, and `lto = "fat"` in `Cargo.toml`.

Not done: thin LTO, `ld64.lld`, sccache (its GHA backend competes for the same
10 GB budget), cross-building x86_64 on Apple Silicon, `--bin touchHLE`.

---

## 3. OPEN: Minigore x-ray sprites and black shaders

### 3.1 Symptom and repro

`com.chillingo.minigore` (Minigore 7.0, armv7, iPhone3GS, 320×480) renders
see-through ("x-ray") sprites and black shading. Confirmed with:

- Renderer: **ANGLE** (bundled), context created as "Native OpenGL ES 1.1"
  (`GLES1Native` on top of ANGLE).
- **Force Composition: ON**.
- Device: Qualcomm Adreno 725, Vulkan 1.1.128, driver 512.615.95, ANGLE 2.1.1
  git hash **`7865acacbf63`**.

The user's own framing: *"before I made modifications, this was not a problem"* —
i.e. this is a regression against the builds they used before this fork's
changes.

### 3.2 What the log already proved

- Force Composition is genuinely active and works structurally:
  `Force Composition is enabled; bypassing the fullscreen EAGL fast path.` then
  `EAGL presenter: no fullscreen layer found (composition is forced by
  --force-composition); … via RAM readback + Core Animation compositing.`
  So every frame goes readback → RGBA → compositor.
- It was **on without being in either options file** ("No options found for this
  app" in both). It comes from the app picker: `PickerSettings::load()` seeds
  `force_composition` from `options.force_composition`, and the Quick Options
  switch persists it in **`touchHLE_picker_settings.txt`** in the same `files/`
  directory (`src/environment/app_picker.rs` around lines 468/477/526). Checking
  the two options files is not enough to know whether it is on.
- Host driver does **not** advertise `GL_IMG_texture_compression_pvrtc`
  (`GLES1Native: … NOT advertised by host driver`), so PVRTC goes through the
  emulator's software decode. First compressed upload:
  `glCompressedTexImage2D(1024x1024, internalformat=0x8c00, image_size=524288)` —
  RGB PVRTC 4bpp.

### 3.3 What was ruled out

- **The fork's own commits do not touch the GL path.** Comparing fork `trunk`
  against upstream `trunk` (`3f7325d`), the net diff was 14 files, of which the
  graphics-relevant ones were:
  - `src/frameworks/opengles/**`, `src/gles/**`, `src/gles.rs` — **identical**
    (all PVRTC decode, `fix_texture_min_filter`, ES2 presenter, shader handling
    is upstream code);
  - `src/window.rs` — 27 lines, `should_attempt_android_angle(gles_native: bool)`
    → `(renderer_mode: RendererMode)`; `!gles_native` and
    `renderer_mode == Angle` are the same predicate, plus a unit test;
  - `src/options.rs` — `gles_native: bool` (default `false` = ANGLE) →
    `renderer_mode: RendererMode` (default `Angle`); the legacy
    `--gles-native` / `--no-gles-native` switches map to the same two states;
  - `src/frameworks/core_animation/ca_eagl_layer.rs` — **3 lines**, a `log_once!`;
  - the rest is app picker / web view / movie player / `ui_touch` UI work.
  There is also **no `renderer_mode`/`gles_native` check anywhere** outside
  `window.rs`, `options.rs` and the picker — the emulator never branches on the
  chosen renderer.
- **"PVRTC decode leaves alpha at zero"** is ruled out for RGB formats:
  `decode_pvrtc_with_alpha(..., is_opaque = true)` ORs `0xFF00_0000` into every
  pixel (`src/image.rs`). Both the `TexImage2D` and `TexSubImage2D` paths use it.
- The Vulkan renderer mode was added and reverted upstream of the merge
  (`f054232`/`e913983` → `bf34051`), net zero.

### 3.4 Leading hypothesis (NOT yet proven)

The bundled ANGLE is **not a dependency — it is four committed binaries** in
`android/app/jniLibs/arm64-v8a/*.so`. The parent repo bumped them on
**2026-10-17** (`b786f84`: `927129a81b04` → `7865acacbf63`, i.e. 2026-09
mainline → 2026-10-16 mainline, **316 upstream ANGLE commits**). The user's
current build reports `7865acacbf63`. That bump is the only vendored-binary or
submodule change on upstream in the preceding weeks, and it arrived through merge
commits, not through the fork's own work.

Those 316 commits are heavily skewed to Vulkan image/texture handling
(`Vulkan: Don't ghost images if base level has changed`, `Vulkan: Never recreate
image due to sRGB override`, `Always robust init all levels of immutable
textures`, `GL: revise size checks for compressed texture workarounds`,
`GL: apply host-twiddled TexSubImage2D workaround to all formats`, `Vulkan:
Custom VMA pools for image suballocation`, …). Adreno runs ANGLE over Vulkan, and
this app feeds that path a software-decoded PVRTC atlas that the guest updates
with many partial `glCompressedTexSubImage2D` calls. Garbage texels in such an
atlas would look exactly like x-ray sprites and black shading.

### 3.5 The experiment that settles it

A one-variable APK already exists: **commit `6d002ed`** restores the 2026-09
ANGLE binaries and changes nothing else. It was built by CI in
**run `40284989655`**, artifact **`HyperHLE_Android_AArch64`** (~46 MB). That
artifact is the only easy way to run the old ANGLE now that `trunk` carries the
new one.

Protocol:

1. Install that APK and confirm the log's second line shows `6d002ed`.
2. Confirm `Driver info: … ANGLE 2.1.1 git hash: ` now reads
   **`927129a81b04`**. If it still says `7865acacbf63`, the rollback did not take
   and the run proves nothing.
3. Run Minigore with ANGLE + Force Composition, same scene as the failing log.

Interpretation:

- **X-ray/black gone or clearly different** → the bundled ANGLE update is the
  cause; the emulator and the fork's commits are exonerated. Options then:
  pin ANGLE by reverting `b786f84`'s binaries (they are 4 files; sizes below),
  take a newer ANGLE build, or work around the specific upload pattern in the
  emulator.
- **Unchanged** → ANGLE is not the variable. Next suspects, in order: the
  software PVRTC path feeding that atlas (per-format upload logging, sub-upload
  handling), `fix_texture_min_filter`, fixed-function blend/dither state the GPU
  presenter perturbs, and the readback/composite path itself.

Binary sizes to tell the two ANGLE builds apart without running them:

| File | 2026-09 (`927129a81b04`) | 2026-10 (`7865acacbf63`) |
|---|---|---|
| `libEGL_angle.so` | 309,888 | 309,712 |
| `libGLESv1_CM_angle.so` | 296,856 | 296,696 |
| `libGLESv2_angle.so` | 6,418,360 | 6,470,584 |
| `libfeature_support_angle.so` | 338,480 | 338,320 |

To rebuild the experiment from a full clone, either
`git checkout b786f84^ -- android/app/jniLibs/arm64-v8a` (needs history:
`git fetch --unshallow`), or `git fetch origin tag v1.1.2` and
`git checkout v1.1.2 -- android/app/jniLibs/arm64-v8a` — `v1.1.2` (2026-10-15) is
25 commits *before* the bump. In both cases verify the sizes against the table.

Do **not** merge such a commit. The previous attempt was reverted (`ec989ed`)
before the merge for exactly that reason.

### 3.6 Comparison build the user may test

The parent repo's **latest** release (`v1.2.10`, 2026-10-25) already contains the
new ANGLE, so it is not a useful comparison. If a release-based comparison is
wanted, use **`v1.1.2`** (2026-10-15) instead — but note it also lacks everything
else upstream did in October, so it only corroborates; the `6d002ed` APK is the
clean test.

### 3.7 Android logging facts that keep coming up

- Log path: `/storage/emulated/0/Android/data/<package>/files/touchHLE_log.txt`;
  `<package>` is `org.hyperhle.reborn` or `org.hyperhle.reborn.unofficial` for
  branded/CI builds (`applicationIdSuffix` in `android/app/build.gradle`).
- The file is **truncated on every launch** (`File::create` in `src/log.rs`), and
  the crash handler writes into the same handle. Pull it after each run.
- 25 entries in `touchHLE_default_options.txt` already pass
  `--force-composition` (plus one comment line), so "force_composition=true" in a
  log does not mean the tester set it. Minigore is **not** in that list.
- Useful diagnostics: `--verbose-gles` (heavy), `TOUCHHLE_TRACE_TEX=1` (names
  every texture upload's format/size), `TOUCHHLE_DUMP_TEX=1` (writes RGB-only PPMs
  of the first 8 PVRTC sub-updates — cannot show alpha, so it is the wrong tool
  for a transparency question).

---

## 4. Measurements (facts, not projections)

Baseline, earlier `trunk` runs (`build-osx (x86_64)` warm 23.8 min; `Build` step
13.35 warm / 25.78 cold):

| Job | baseline | run `40281051199` (PR, `c4068ce`) | run `40294335854` (PR, `ec989ed`) |
|---|---|---|---|
| build-osx (x86_64) | 23.8 warm / 39.3 cold | 25.4 (`Build` 23.2, cache miss) | green, all 7 jobs |
| build-win | 12.7 | 15.6 | green |
| **test-macos** | — (tests were inside build-osx) | **13.3** (`Test` 10.4) | green |
| build-osx (aarch64) | 11.9 | 12.4 | green |
| build-ios | 12.6 | 10.2 | green |
| build-linux | 5.7 | 8.0 | green |
| build-android | 6.8 | 5.9 | green |

Post-merge `trunk` pushes:

| Run | Commit | Result | Notes |
|---|---|---|---|
| `40299058134` | `bee1ffc` (the merge) | **failure** | only `build-android` failed, at step "Build for Android" (exit 1), 4.2 min in; everything else green; `publish` skipped |
| `40305274141` | `0aa2646` | success | all jobs green, **published `v1.0.11`** |

Timings from `40305274141`, the first `trunk` run with cache saving re-enabled:
`build-osx (x86_64)` **22.4** min (`Build` 20.28, cache restore 0.52),
`test-macos` 15.5, `publish` **13.7**, `build-ios` 12.9, `build-win` 10.8,
`build-osx (aarch64)` 10.4, `build-linux` 8.0, `build-android` 6.9.

Three things to take from that:

- **The Android failure on the merge commit looks like a flake, not a
  regression** — the identical Android code passed in both PR runs
  (`40281051199`, `40294335854`), in the TEMP-ANGLE run (`40284989655`) and on
  the next trunk push. Job logs are not retrievable from this sandbox (blob
  storage `EOF`, see §6), so this is unconfirmed: open
  <https://github.com/Zanuaimi/HyperHLE-Reborn-Experimental/actions/runs/40299058134>
  and read the "Build for Android" step before believing either explanation.
- **The 3a projection is still not demonstrated.** This was the first run where
  caches could be written at all, and `Build` came in at 20.28 min, not the
  projected ~12–13. A cache hit is plausible (0.52 min restore) but not proven,
  and one sample cannot separate `codegen-units`, `CMAKE_BUILD_PARALLEL_LEVEL`
  and cache luck on a runner whose `Build` step has ranged 13–26 min for the same
  commit graph. `--timings` is the way to settle it.
- **`publish` is now a real long pole**: 13.7 min, of which **11.87 min is
  `Install packaging tools`** (`sudo apt-get update && apt-get install pandoc
  zip`). That is runner/mirror slowness rather than something these changes
  introduced, but it now dominates the tail: workflow wall-clock is roughly
  `build-osx (x86_64)` 22.4 + `publish` 13.7 ≈ 36 min. Caching or preinstalling
  `pandoc`, or installing it while the builds run, is probably worth more than
  any remaining `codegen-units` tuning.

Both PR runs skipped `check-hyperhle-release` and `publish-hyperhle-release`,
which is correct on a pull request. `test-macos` is the first CI compile of the
EAGL change **and it runs TestApp**, so `AGENTS.md`'s "say TestApp was not run"
caveat no longer applies to it: it ran, green, twice.

Cache inventory before this work: **39 entries / 10.59 GB against a 10 GB
limit** — hence F2 in the plan doc (eviction turns the `Build` step from 13 into
26 minutes).

---

## 5. Open items

1. **Minigore** (§3) — highest priority; everything else is polish.
2. **`codegen-units` gain is still unmeasured.** Both PR runs were cache misses.
   The first cache-saving `trunk` run gave `Build` 20.28 min (see §4), which does
   not support the projected 12–13 min. Measure with `workflow_dispatch` +
   `ci_timings` ticked and record the split in the plan doc §6. Be willing to
   conclude the change was neutral or unhelpful and revert it — the honest
   outcome matters more than the projected one.
3. **`publish` / `Install packaging tools`** (§4): 11.87 min of `apt-get` for
   `pandoc zip` *after* all builds finish. Cache or preinstall `pandoc`, or
   overlap the install with the builds. Biggest remaining win if the numbers
   hold.
4. **F9**: whether a second fat-LTO cdylib link exists. `--timings` answers it;
   if it does, `cargo build --release --bin touchHLE` would drop it.
5. **F7 at level 4**: `CMAKE_BUILD_PARALLEL_LEVEL` is deliberately conservative
   (2) because cargo can run several build scripts at once; 4 is the documented
   next experiment.
6. **`build-win` / `build-ios`** become the critical path once macOS drops below
   ~12.6 min; neither has had a `--timings` run.
7. The plan doc's §8 checklist tracks the above; keep it current rather than
   adding a second list.

---

## 6. Traps (things that were tried and rejected)

- **Do not** reintroduce force-composition into present-route selection. The
  earlier "make `--force-composition` select the presentation route" change was
  based on the false claim that the switch was inert; it bypasses the fast path
  entirely and needs no plumbing.
- **Do not** add an explicit `key:` to `Swatinem/rust-cache` — it would discard
  the warm entries (one cold ~26-minute build).
- **Do not** "fix" the `hdiutil` retry loop or shorten its back-off; it only runs
  after `hdiutil create` has already failed.
- **Do not** make the `trunk` concurrency group cancel in progress.
- **Do not** claim the iOS SDL build is serial — it uses `-G Ninja`.
- **Do not** weaken or delete a test to get green. If TestApp cannot run, say so
  in the PR; here it does run, in `test-macos`.
- Environment limits seen in this session: no Rust toolchain, so `cargo
  build`/`test`/`clippy`/`doc` cannot be run locally — CI is the gate.
  `gh run view --job … --log` fails (blob-storage `EOF`) and artifact downloads
  can fail the same way; use the Jobs API (`gh api …/actions/runs/<id>/jobs`) and
  let the user download artifacts in the browser.
- `pip install pyyaml` needs `--break-system-packages` here; YAML-validate any
  workflow edit before pushing.
- `dev-scripts/lint.sh` enforces ≤ 80 columns on single-line `//` comments
  (URLs exempt). The repo already has 4 violations in `eagl.rs`; do not add more.

---

## 7. Quick reference

| Thing | Value |
|---|---|
| Merged PR | #1, merge commit `bee1ffc`, merged 2026-10-26T09:22Z |
| `trunk` head | `0aa2646` |
| EAGL diagnostics commit | `53c412e` (+ `c4068ce`… review fixes) |
| Workflow commits | `8ab1b9e`, `6208b39`, `1f51777`, `9eb2729`, `c4068ce`, `e950e99` |
| TEMP ANGLE rollback / revert | `6d002ed` / `ec989ed` (content-neutral) |
| Experiment APK | run `40284989655`, artifact `HyperHLE_Android_AArch64` |
| PR CI runs | `40281051199` (`c4068ce`), `40294335854` (`ec989ed`) — both green |
| Post-merge trunk runs | `40299058134` (`bee1ffc`) — `build-android` failed (likely flake); `40305274141` (`0aa2646`) — green, published `v1.0.11` |
| Plan doc | `dev-scripts/ci-performance-macos-x86_64.md` |
| Parent repo | `KlugKlugTG/HyperHLE-Reborn`, upstream `trunk` was `3f7325d` / `v1.2.10` |
