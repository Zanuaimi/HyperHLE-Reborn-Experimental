# HyperHLE-Reborn Compatibility TODO

Prioritize accurate, shared iOS behavior and verify changes with reproducible tests. Keep game-specific workarounds in options rather than bundle-ID checks in emulator code.

## Compatibility test coverage

- [ ] Build a small, versioned compatibility suite covering representative iOS 2–10 app behaviors: launch, touch input, timers, animation, audio, movie playback, and rendering.
- [ ] For each test, record the iOS/API behavior it covers and add a regression test when a compatibility issue is fixed.

## Graphics and Minigore

- [ ] Reproduce Minigore 1’s black/x-ray shader issue during gameplay with ANGLE and Force Composition; record the exact steps and renderer configuration.
- [ ] Capture and compare useful graphics state for the failing case: shader compile/link results, texture formats and dimensions, framebuffer setup, blend/depth state, and presentation route.
- [ ] Use the evidence to identify and fix the underlying GLES/EAGL behavior; verify clean gameplay and add a regression test where practical.

## Core Animation and timed UI

- [ ] Review Core Animation behavior against real app usage, prioritizing keyframe animations, animation groups, presentation-layer values, timing/repeat behavior, and CADisplayLink cadence.
- [ ] Add focused tests for animated UI patterns such as sprites, loading indicators, labels, menus, and transitions.

## Gun Bros 2 loading and movie playback

- [ ] Trace Gun Bros 2’s loading sequence, including run-loop progress, timers, display links, and animation callbacks, to determine why its loading UI appears static.
- [ ] Trace the intro movie lifecycle from loading through first frame, layout, notifications, and completion; identify why loading fails and why the expected presentation (including black bars) is missing.
- [ ] Add movie-player lifecycle tests for state notifications, seeking, completion, looping, fullscreen transitions, scaling modes, and aspect-ratio/background handling.
- [ ] Verify that Gun Bros 2 loads successfully; record the tested app/device configuration and add a regression test where practical.

## Audio compatibility

- [ ] Add Audio Queue/Audio Unit behavior tests for start, stop, pause, buffer callbacks, format changes, interruption/resume, and simultaneous sound/music.
- [ ] Use failures to identify and correct lifecycle or format mismatches that cause older apps to be silent or behave incorrectly.
