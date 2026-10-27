/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Per-frame tally of what the guest app asked OpenGL ES to do.
//!
//! Every "the picture is black / see-through" report has two very different
//! explanations, and until now the log could not tell them apart without a
//! rebuild:
//!
//! 1. the guest really did draw almost nothing (a stubbed entry point, a
//!    feature test the guest failed, a scene skipped because of an error), or
//! 2. the guest drew a whole scene and the result is being mangled on the way
//!    to the screen (wrong state, wrong texture contents, wrong presentation).
//!
//! Counting draws and vertices between two presented frames separates them in
//! one line, and it costs three relaxed atomic adds per draw: no GL calls, no
//! allocations, nothing that can perturb the thing being measured.
//!
//! The tally covers what the guest asked for, not what the emulator draws: the
//! compositor and the presenters call the GLES layer directly and are not
//! counted. It includes the `glDrawTex*OES` shortcut, which draws a rectangle
//! without going through `glDrawArrays`/`glDrawElements`.
//!
//! The counters are consumed by `probe_readback_content`, which runs on the
//! RAM-readback present routes where the frame is already in memory, and
//! therefore needs no diagnostic switch to be turned on first.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static DRAWS: AtomicU64 = AtomicU64::new(0);
static VERTICES: AtomicU64 = AtomicU64::new(0);
static TEXTURE_UPLOADS: AtomicU64 = AtomicU64::new(0);
/// Bitwise OR of every `glClear` mask the guest passed since the previous
/// present. Recorded rather than counted: what matters is *which* buffers were
/// cleared, not how many times.
static CLEAR_MASKS: AtomicU64 = AtomicU64::new(0);

/// The guest's own `GL_DEPTH_TEST` request, for the drawable-capability line.
///
/// The host's depth-test bit is not the guest's: depth testing is forced off
/// while a framebuffer without a guest-attached depth buffer is bound, and a
/// diagnostic that reported the host bit would then contradict the app's own
/// state. Diagnostic only — nothing in the renderer reads this.
static GUEST_DEPTH_TEST: AtomicBool = AtomicBool::new(false);

/// `GL_COLOR_BUFFER_BIT`, `GL_DEPTH_BUFFER_BIT` and `GL_STENCIL_BUFFER_BIT`.
const CLEAR_COLOR: u64 = 0x4000;
const CLEAR_DEPTH: u64 = 0x0100;
const CLEAR_STENCIL: u64 = 0x0400;

/// What the guest submitted since the previous present.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameWorkload {
    /// `glDrawArrays` / `glDrawElements` / instanced variants.
    pub draws: u64,
    /// Sum of the `count` arguments of those calls.
    pub vertices: u64,
    /// `glTexImage2D` / `glTexSubImage2D` / `glCompressedTexImage2D`.
    pub texture_uploads: u64,
    /// Whether any `glClear` in this frame included `GL_COLOR_BUFFER_BIT`.
    pub cleared_color: bool,
    /// Whether any `glClear` in this frame included `GL_DEPTH_BUFFER_BIT`.
    ///
    /// This is the datum for the "the depth buffer is never cleared" family of
    /// bugs: a guest that enables depth testing but only ever clears colour
    /// relies on the driver starting each frame with a clean depth buffer (or on
    /// there being no depth buffer at all).
    pub cleared_depth: bool,
    /// Whether any `glClear` in this frame included `GL_STENCIL_BUFFER_BIT`.
    pub cleared_stencil: bool,
}

impl FrameWorkload {
    /// True if the guest submitted no draw calls at all.
    pub fn drew_nothing(&self) -> bool {
        self.draws == 0
    }
}

/// Count one draw call of `vertices` vertices.
pub fn record_draw(vertices: u64) {
    DRAWS.fetch_add(1, Ordering::Relaxed);
    VERTICES.fetch_add(vertices, Ordering::Relaxed);
}

/// Count one `glDrawTex*OES` call.
///
/// That extension draws a single textured rectangle, which is four vertices
/// as far as the tally is concerned, and it bypasses the draw entry points
/// below, so a 2D engine that renders with it alone would otherwise look like
/// an engine that submitted nothing at all.
pub fn record_quad_draw() {
    record_draw(4);
}

/// Count one texture upload.
pub fn record_texture_upload() {
    TEXTURE_UPLOADS.fetch_add(1, Ordering::Relaxed);
}

/// Record the guest's own `GL_DEPTH_TEST` request.
pub fn record_guest_depth_test(enabled: bool) {
    GUEST_DEPTH_TEST.store(enabled, Ordering::Relaxed);
}

/// The last `GL_DEPTH_TEST` request the guest made.
pub fn guest_depth_test() -> bool {
    GUEST_DEPTH_TEST.load(Ordering::Relaxed)
}

/// Record one `glClear` mask.
pub fn record_clear(mask: u64) {
    CLEAR_MASKS.fetch_or(mask, Ordering::Relaxed);
}

/// Take the counters accumulated since the previous call, resetting them.
pub fn take_frame_workload() -> FrameWorkload {
    let clears = CLEAR_MASKS.swap(0, Ordering::Relaxed);
    let (cleared_color, cleared_depth, cleared_stencil) = clear_flags(clears);
    FrameWorkload {
        draws: DRAWS.swap(0, Ordering::Relaxed),
        vertices: VERTICES.swap(0, Ordering::Relaxed),
        texture_uploads: TEXTURE_UPLOADS.swap(0, Ordering::Relaxed),
        cleared_color,
        cleared_depth,
        cleared_stencil,
    }
}

/// Split an OR of `glClear` masks into (colour, depth, stencil) flags.
fn clear_flags(mask: u64) -> (bool, bool, bool) {
    (
        mask & CLEAR_COLOR != 0,
        mask & CLEAR_DEPTH != 0,
        mask & CLEAR_STENCIL != 0,
    )
}

/// Which buffers the frame's `glClear` calls covered, as a log-friendly phrase.
pub fn describe_clears(cleared_color: bool, cleared_depth: bool, cleared_stencil: bool) -> String {
    if !cleared_color && !cleared_depth && !cleared_stencil {
        return "the guest issued no glClear".to_string();
    }
    let mut bits: Vec<&str> = Vec::new();
    if cleared_color {
        bits.push("COLOR");
    }
    if cleared_depth {
        bits.push("DEPTH");
    }
    if cleared_stencil {
        bits.push("STENCIL");
    }
    format!("glClear covered {}", bits.join("|"))
}

/// One-sentence description of a frame's workload, for log lines.
///
/// Kept separate from the logging so the classification can be tested; the
/// interesting boundary is "no draws at all", which is the signature of a
/// scene that was never submitted rather than one that was rendered wrongly.
pub fn describe(workload: FrameWorkload) -> String {
    let verdict = if workload.drew_nothing() {
        "the guest submitted no draws between these presents"
    } else if workload.vertices == 0 {
        "every draw was empty"
    } else {
        "the guest is submitting geometry"
    };
    let FrameWorkload {
        draws,
        vertices,
        texture_uploads,
        cleared_color,
        cleared_depth,
        cleared_stencil,
    } = workload;
    let clears = describe_clears(cleared_color, cleared_depth, cleared_stencil);
    format!(
        "{draws} draws, {vertices} vertices, {texture_uploads} texture uploads \
         since the previous present: {verdict}; {clears}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_each_workload_shape() {
        assert!(describe(FrameWorkload::default()).contains("no draws"));
        assert!(describe(FrameWorkload {
            draws: 3,
            vertices: 0,
            texture_uploads: 1,
            ..Default::default()
        })
        .contains("every draw was empty"));
        let busy = describe(FrameWorkload {
            draws: 120,
            vertices: 45_000,
            texture_uploads: 0,
            ..Default::default()
        });
        assert!(busy.contains("120 draws"));
        assert!(busy.contains("45000 vertices"));
        assert!(busy.contains("submitting geometry"));
    }

    #[test]
    fn take_resets_the_counters() {
        // The counters are process-global, so this test must not assume they
        // start at zero; it only checks that taking them empties them.
        record_draw(6);
        record_draw(3);
        record_quad_draw();
        record_texture_upload();
        record_clear(CLEAR_COLOR);
        let first = take_frame_workload();
        assert!(first.draws >= 3);
        assert!(first.vertices >= 13);
        assert!(first.texture_uploads >= 1);
        assert!(first.cleared_color);
        assert!(!first.cleared_depth);
        assert!(!first.drew_nothing());
        assert_eq!(take_frame_workload(), FrameWorkload::default());
    }

    #[test]
    fn describes_which_buffers_were_cleared() {
        assert_eq!(
            describe_clears(false, false, false),
            "the guest issued no glClear"
        );
        assert_eq!(describe_clears(true, false, false), "glClear covered COLOR");
        assert_eq!(
            describe_clears(true, true, false),
            "glClear covered COLOR|DEPTH"
        );
        assert_eq!(
            describe_clears(true, true, true),
            "glClear covered COLOR|DEPTH|STENCIL"
        );
        // A colour clear and a separate depth clear in the same frame OR
        // together, which is what "any glClear covered it" means. Checked on
        // the mask directly rather than through the process-global counters,
        // so this test cannot race the others.
        assert_eq!(clear_flags(CLEAR_COLOR | CLEAR_DEPTH), (true, true, false));
        assert_eq!(clear_flags(CLEAR_COLOR), (true, false, false));
        assert_eq!(clear_flags(0), (false, false, false));
    }
}
