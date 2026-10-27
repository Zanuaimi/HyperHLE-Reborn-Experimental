# Session handoff — 2026-10-27

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
| Rendering fidelity fixes (`src/gles/**`, `src/frameworks/opengles/**`) | **Pushed, not compiled** — commits `babfe011`, `271d0c42`, `6dc6e2c2`, `262d47e4` on `arena/16e7a9d5-hyperhle-reborn-experimental` |
| Minigore x-ray sprites / black shaders | **Open, root cause not proven**; reproduces on upstream, so it is not a fork regression |

Local `trunk` is `6fab5922` (= upstream `v1.2.12`'s merge base), and the session
branch is that plus the four commits above. Upstream published **`v1.2.12`** on
2026-10-27T07:01:39Z; that is the build the newest diagnostic log came from.

**Nothing in this tree since `babfe011` has been compiled.** There is no Rust
toolchain in the sandbox the work was done in, so those commits are
inspection-reviewed only. First job on a machine with a toolchain:
`dev-scripts/lint.sh` (clippy `--deny warnings`), then
`cargo test -- --skip test_app`.

The open investigation is worth reading first (§3) because it is the only thing
that still needs work.

### Checkout quirks you will hit immediately

- **This clone is shallow**: `.git/shallow` contains `0aa2646`, so `git log`
  shows a single commit. Run `git fetch --unshallow origin` before any history
  archaeology.
- **There is no `upstream` remote**, only `origin`
  (`Zanuaimi/HyperHLE-Reborn-Experimental`). To compare against the parent repo:
  `git remote add upstream https://github.com/KlugKlugTG/HyperHLE-Reborn.git`.
- Work on the session branch only
  (`arena/16e7a9d5-hyperhle-reborn-experimental`); do not push elsewhere.
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

### 2.3 Diagnostics added on 2026-10-27

Three changes, all in `src/frameworks/opengles/eagl.rs` plus a portable dump
path in `src/paths.rs`. None of them is compiled yet.

1. `probe_readback_content` — the RAM-readback present route now reports what
   the presented frame contains (four corners, centre, how much of the frame is
   flat or black) on the same frame schedule as the GPU-route probe. Gated on
   the picker's "Trace GL errors" switch. Verdict logic lives in
   `frame_content_verdict`, which has unit tests; it reports `unknown` for an
   empty sample set rather than calling it `flat`.
2. `drain_startup_gl_errors` — drains the host GL error queue once, right after
   the guest's context is created and before the guest can make its own GL call,
   so touchHLE's own start-up probes cannot be mistaken for the app's errors,
   and logs when it finds anything. **This is not proven to explain the `0x500`
   in the Minigore log** — see §3.2; the trace already clears the queue before
   each traced dispatch, which points the other way. It is hygiene, plus a way
   to convert the ambiguity into evidence on the next run.
3. `paths::diagnostic_dump_path` — all diagnostic PPM dumps now land in
   `<user data>/touchHLE_dumps/` beside `touchHLE_log.txt` instead of
   hardcoded `/tmp/a8run/...` paths.

### 2.4 Review pass over the uncompiled rendering work (2026-10-27)

A read-through of everything since `babfe011`, looking for the kind of defect
that only a compiler or a driver would catch. Still uncompiled. Four real bugs
found and fixed, all in the NPOT texture workarounds:

1. `gles1_native` queried `GL_TEXTURE_MIN_FILTER` with `glGetIntegerv`. That enum
   is per-texture-object state and is absent from ES 1.1's `glGetIntegerv` table,
   so ANGLE (the default Android renderer) answers `GL_INVALID_ENUM` and leaves
   the output at zero — the filter was never degraded, and the failed query put a
   stray error in the guest's queue. Now `glGetTexParameteriv`, as the other two
   backends already did.
2. Every fix-up named `GL_TEXTURE_2D` explicitly, so cube-map face uploads
   adjusted (or read) the wrong texture object. All go through
   `util::tex_parameter_target` now.
3. The wrap-mode fix-up ran only for `glTexImage2D`. PVRTC arrives via
   `glCompressedTexImage2D`, so the fix missed what iOS games actually upload.
   It now runs on uncompressed, compressed and `glCopyTexImage2D` paths (the
   desktop-GL backend keeps skipping the wrap forcing on purpose).
4. `gles1_on_gl2`'s filter fix-up ignored its `target`.

Two smaller things worth remembering: `GlobalGenerator` (which these bindings
use) does emit a module per *extension* function, so `gles11::FooOES::is_loaded()`
is valid — that idiom in `supports_vao_oes` is fine. And `glGetString` in
`gles_guest` keys its cache on `(is_es2, matrix_palette, name)`, so any backend
that changes what it advertises must return a stable answer for the session or
the cache has to be invalidated.

## 3. OPEN: Minigore x-ray sprites and black shaders

### 3.1 Symptom and repro

`com.chillingo.minigore` (Minigore 7.0, armv7, iPhone3GS, 320×480) renders
see-through ("x-ray") sprites and black shading. Confirmed with:

- Renderer: **ANGLE** (bundled), context created as "Native OpenGL ES 1.1"
  (`GLES1Native` on top of ANGLE's GLES1 front end).
- **Force Composition: ON**.
- Host: Qualcomm Adreno 725, Vulkan 1.1.128, driver 512.615.95, ANGLE 2.1.1
  git hash **`7865acacbf63`**.

It reproduces on the **parent repo's own release `v1.2.12`**
(`org.hyperhle.reborn.unofficial`, published 2026-10-27T07:01:39Z), so it is not
a regression introduced by this fork. Do not re-open that theory.

### 3.2 What the `v1.2.12` log established

- Every frame is presented through the **RAM readback → Core Animation**
  route: `EAGL presenter: no fullscreen layer found (composition is forced by
  --force-composition); presenting renderbuffer 0x1 … via RAM readback (slow
  path)`.
- Host ES 1.1 driver is ANGLE: `OpenGL ES 1.1 (ANGLE 2.1.1 git hash:
  7865acacbf63) / Google Inc. (Qualcomm) / ANGLE (Qualcomm, Vulkan 1.1.128
  (Adreno 725), Driver-512.615.95)`.
- `GL_IMG_texture_compression_pvrtc NOT advertised by host driver` → PVRTC is
  software-decoded, as designed. `fix_texture_min_filter: ON`.
- The steady-state frame loop repeats verbatim, and it is **pure 2D
  compositing**: `glGetIntegerv(0x8894)`, `glGetIntegerv(0x8895)`,
  `glViewport(0,0,640,960)`, `glClear(0x4000)`, bind texture 3 (320×480) +
  `glDrawElements(GL_TRIANGLES, 6, UNSIGNED_SHORT)`, bind texture 5 (320×480) +
  the same draw, then `glViewport(390,0,1620,1080)`, bind texture 1 (640×960) +
  `glDrawArrays(GL_TRIANGLES, 0, 6)`. Two triangles per pass. No 3D draw calls,
  no lighting/fog/matrix-palette traffic, no `glDepthFunc`, no `glBlendFunc`,
  and `glClear` is called with `0x4000` (colour only) after the first frame.
- One `GL Error: 0x500`, reported against the very first
  `glGetIntegerv(0x8894)` (`GL_VERTEX_ARRAY_BINDING`); every later call reports
  `0x0`. **Two readings are still possible and the v1.2.12 log cannot separate
  them.** `with_ctx_and_mem` already drains the queue before each traced
  dispatch, which should mean the call itself raised the error — i.e. the app
  probed an ES 2.0/OES enum on an ES 1.1 context, which raises
  `GL_INVALID_ENUM` on a real device too and is not an emulator defect. But
  `with_ctx_and_mem_no_skip` (3 call sites, buffer mapping) does *not* drain
  first, so a stale error would be blamed on whichever call came next. The
  2026-10-27 trace change makes the two cases print differently; the next log
  settles it.
- **The `renderbuffer-content probe` line never appeared even though the probe
  is gated on the `--trace-gl-errors` switch that was on.** It lives inside the
  GPU-present branch, which this route never enters, so the readback path had no
  pixel-level diagnostic at all. Fixed on 2026-10-27.

### 3.3 What was ruled out

- **The fork's own commits as the cause** — it reproduces on upstream `v1.2.12`.
- **A missing depth attachment / depth-test absence as the cause** — the
  steady-state frame issues no depth state at all: no `glDepthFunc`, no
  `0x4100` clear, no depth-tested geometry. Whatever the x-ray look is, it is
  produced by the two-quad 2D path above.
- **`GL_TEXTURE_MIN_FILTER` left at the mipmapping default** — two independent
  reasons. `glTexParameteri` logs only the *first* call of the whole session
  (`First glTexParameteri(…)[this log will only be shown once]`), so the absence
  of a `0x2801` line proves nothing; and when the guest never sets it,
  `fix_texture_min_filter` forces `GL_LINEAR` after every level-0 upload anyway
  (`First fix_texture_min_filter override`, printed once).
- **"PVRTC decode leaves alpha at zero"** for RGB formats:
  `decode_pvrtc_with_alpha(..., is_opaque = true)` ORs `0xFF00_0000` into every
  pixel (`src/image.rs`); both the `TexImage2D` and `TexSubImage2D` paths use it.
- The Vulkan renderer mode was added and reverted upstream of the merge
  (`f054232`/`e913983` → `bf34051`), net zero.

### 3.4 Open hypotheses, in order

1. **The displayed frame is what the guest drew, and the guest drew almost
   nothing.** If the log was captured during gameplay, two full-screen textured
   quads per frame is not a 3D scene: the world is not being drawn at all and
   the visible "x-ray" layer is whatever UI/compositor quads survive. That is a
   drawing problem (state, feature detection, a stubbed entry point), not a
   presentation problem. The new readback probe separates this from (2) in one
   line.
2. **The readback → compositor path changes the picture.** The direct route
   hands the frame to the platform swap chain, which ignores alpha; the
   composition route blends it with Core Animation's **premultiplied** contract
   (`ONE, ONE_MINUS_SRC_ALPHA`) whenever the layer is not opaque —
   `CALayer.opaque` defaults to `NO` here, while `UIView.opaque` defaults to
   `YES` on iOS. `log_nonopaque_present_once` already reports how much of the
   frame's alpha is below 255, which is the deciding datum.
3. **ANGLE's GLES1 front end mishandling this app's ES 1.1 state** (the vendored
   ANGLE was bumped 2026-10-17, `927129a81b04` → `7865acacbf63`, 316 upstream
   commits, heavily Vulkan texture-path churn). Still untested; the one-variable
   APK below is the way to test it, but it is now the *third* suspect, not the
   first.

### 3.5 Next experiments

1. **Read the new probe line.** Any build from 2026-10-27 onward prints, on the
   readback route and with Trace GL errors on:

       [--trace-gl-errors] present_renderbuffer readback content probe
       (frame=N, WxH): BL=(r,g,b,a) BR=… TL=… TR=… CENTER=… |
       n/total sampled pixels match CENTER, m/total are black | verdict

   `verdict` is `all black`, `flat`, `content present`, or `unknown`. Grep:

       grep -iE "readback content probe|renderbuffer-content probe|eagl presenter|non-opaque layer"

   - `all black` / `flat` → hypothesis (1): the frame really is empty, look at
     what the guest is (not) drawing.
   - `content present` → hypothesis (2): compare a screenshot with the buffer.
     Buffer right + screen wrong = presentation path; both wrong = upstream.
2. **Watch how the trace attributes errors.** Two new lines exist:
   "Drained GL errors raised by touchHLE's own start-up probes…" (once, right
   after the context is created) and
   "[--trace-gl-errors] … had glGetError() = 0xNNN pending before a call
   dispatched from file:line; that error predates this call". If the `0x500` of
   the v1.2.12 log comes back as the *second* form, it is stale and mine; if it
   comes back attached to a call with no pending-prefix line before it, the app
   raised it itself (see §3.2) and it should be left alone.
3. **Ask for these greps from a fresh log**: `matrix palette`,
   `not supported by this backend`, `First fix_texture_min_filter`,
   `First glTexParameteri`. Together they say whether the app took a code path
   the renderer cannot serve.
4. **Only if the above is inconclusive: the ANGLE A/B.** Commit `6d002ed`
   (never merged) restores the 2026-09 ANGLE binaries `927129a81b04` and changes
   nothing else; CI run `40284989655` built it (artifact
   `HyperHLE_Android_AArch64`). Confirm the second log line shows `6d002ed` and
   that `Driver info:` reads `927129a81b04` before drawing any conclusion.

Binary sizes to tell the two ANGLE builds apart without running them:

| File | 2026-09 (`927129a81b04`) | 2026-10 (`7865acacbf63`) |
|---|---|---|
| `libEGL_angle.so` | 309,888 | 309,712 |
| `libGLESv1_CM_angle.so` | 296,856 | 296,696 |
| `libGLESv2_angle.so` | 6,418,360 | 6,470,584 |
| `libfeature_support_angle.so` | 338,480 | 338,320 |

To rebuild that experiment from a full clone, either
`git checkout b786f84^ -- android/app/jniLibs/arm64-v8a` (needs history:
`git fetch --unshallow`), or `git fetch origin tag v1.1.2` and
`git checkout v1.1.2 -- android/app/jniLibs/arm64-v8a` — `v1.1.2` (2026-10-15)
is 25 commits *before* the bump. Verify the sizes against the table. Do **not**
merge such a commit; the previous attempt was reverted (`ec989ed`).

### 3.6 Android logging facts that keep coming up

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
- **Environment variables are unreachable on Android** (there is no shell to set
  them from), so anything gated on `TOUCHHLE_*` is developer-only. The picker's
  **"Trace GL errors"** switch (`src/environment/app_picker.rs`, persisted in
  `touchHLE_picker_settings.txt` as `trace_gl_errors=`) is the only diagnostic a
  tester can turn on. Instrumentation meant for testers belongs behind that
  switch, not behind an env var.
- As of 2026-10-27, PPM dumps from `TOUCHHLE_DUMP_READBACK`,
  `TOUCHHLE_DUMP_TEX` and the ES 2.0 post-draw capture are written to
  `<files>/touchHLE_dumps/` instead of `/tmp/a8run/` (which does not exist on
  Android or iOS, so those dumps had been failing silently on the only platforms
  that produce bug reports).
- Two traps that have already caused wrong conclusions:
  - `glTexParameteri` is logged for its **first call only**, so a missing
    `pname=` line says nothing about whether the app sets that parameter later.
  - The `GL Error: {:#x}` trace line now appears both **before** and **after**
    each traced dispatch (the pre-call one drains the queue). Before
    2026-10-27 the pre-call drain was silent, so a stale error was
    indistinguishable from one the call itself raised. Read the
    "… pending before a call dispatched from" line when present.

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

1. **Minigore** (§3) — highest priority; everything else is polish. The next
   step costs nothing and does not need a build: reproduce with Trace GL errors
   on a build that has the 2026-10-27 commits, then read the
   `Readback content probe` line (§3.5).
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
- **`git diff` of `eagl.rs` is a poor brace counter** — the file is mostly inside
  `objc_classes! { … }`, so pick anchors by their surrounding text and assert on
  the match count before replacing. Two `str.replace` anchors have silently
  matched twice in this session (the two `initWithAPI:` variants).
- Statements about "what the guest never does", when drawn from the log, must
  account for the trace's sampling: `glTexParameteri` prints only its first call
  and the `GL Error:` line prints after each call, not before it.
- **A syntax-only check is possible without a toolchain**:
  `pip install --break-system-packages tree-sitter tree-sitter-rust` and parse
  the changed files for `ERROR`/missing nodes. It catches unbalanced braces and
  broken string continuations, not type errors; `src/objc/messages.rs` has
  known false positives (fn-pointer-type macro patterns).

---

## 7. Quick reference

| Thing | Value |
|---|---|
| Merged PR | #1, merge commit `bee1ffc`, merged 2026-10-26T09:22Z |
| Local `trunk` | `6fab5922` (= upstream `v1.2.12` merge base) |
| Session branch head | `d65f349f` (2026-10-27 review pass) |
| Rendering-fix commits | `babfe011`, `271d0c42`, `6dc6e2c2`, `262d47e4` — **uncompiled** |
| EAGL diagnostics commit | `53c412e` (+ `c4068ce`… review fixes) |
| Workflow commits | `8ab1b9e`, `6208b39`, `1f51777`, `9eb2729`, `c4068ce`, `e950e99` |
| TEMP ANGLE rollback / revert | `6d002ed` / `ec989ed` (content-neutral) |
| Experiment APK | run `40284989655`, artifact `HyperHLE_Android_AArch64` |
| PR CI runs | `40281051199` (`c4068ce`), `40294335854` (`ec989ed`) — both green |
| Post-merge trunk runs | `40299058134` (`bee1ffc`) — `build-android` failed (likely flake); `40305274141` (`0aa2646`) — green, published `v1.0.11` |
| Plan doc | `dev-scripts/ci-performance-macos-x86_64.md` |
| Parent repo | `KlugKlugTG/HyperHLE-Reborn`, latest release `v1.2.12` (2026-10-27T07:01Z) |
