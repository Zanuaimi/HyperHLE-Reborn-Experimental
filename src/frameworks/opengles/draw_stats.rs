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
//! one line, and it costs three relaxed atomic adds per draw — no GL calls, no
//! allocations, nothing that can perturb the thing being measured.
//!
//! The counters are consumed by `probe_readback_content`, which runs on the
//! RAM-readback present routes where the frame is already in memory, and
//! therefore needs no diagnostic switch to be turned on first.

use std::sync::atomic::{AtomicU64, Ordering};

static DRAWS: AtomicU64 = AtomicU64::new(0);
static VERTICES: AtomicU64 = AtomicU64::new(0);
static TEXTURE_UPLOADS: AtomicU64 = AtomicU64::new(0);

/// What the guest submitted since the previous present.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameWorkload {
    /// `glDrawArrays` / `glDrawElements` / instanced variants.
    pub draws: u64,
    /// Sum of the `count` arguments of those calls.
    pub vertices: u64,
    /// `glTexImage2D` / `glTexSubImage2D` / `glCompressedTexImage2D`.
    pub texture_uploads: u64,
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

/// Count one texture upload.
pub fn record_texture_upload() {
    TEXTURE_UPLOADS.fetch_add(1, Ordering::Relaxed);
}

/// Take the counters accumulated since the previous call, resetting them.
pub fn take_frame_workload() -> FrameWorkload {
    FrameWorkload {
        draws: DRAWS.swap(0, Ordering::Relaxed),
        vertices: VERTICES.swap(0, Ordering::Relaxed),
        texture_uploads: TEXTURE_UPLOADS.swap(0, Ordering::Relaxed),
    }
}

/// One-sentence description of a frame's workload, for log lines.
///
/// Kept separate from the logging so the classification can be tested; the
/// interesting boundary is "no draws at all", which is the signature of a
/// scene that was never submitted rather than one that was rendered wrongly.
pub fn describe(workload: FrameWorkload) -> String {
    let FrameWorkload {
        draws,
        vertices,
        texture_uploads,
    } = workload;
    let verdict = if draws == 0 {
        "the guest submitted no draws between these presents"
    } else if vertices == 0 {
        "every draw was empty"
    } else {
        "the guest is submitting geometry"
    };
    format!(
        "{draws} draw call(s), {vertices} vertices, \
         {texture_uploads} texture upload(s) since the previous present: {verdict}"
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
        })
        .contains("every draw was empty"));
        let busy = describe(FrameWorkload {
            draws: 120,
            vertices: 45_000,
            texture_uploads: 0,
        });
        assert!(busy.contains("120 draw call(s)"));
        assert!(busy.contains("45000 vertices"));
        assert!(busy.contains("submitting geometry"));
    }

    #[test]
    fn take_resets_the_counters() {
        // The counters are process-global, so this test must not assume they
        // start at zero; it only checks that taking them empties them.
        record_draw(6);
        record_draw(3);
        record_texture_upload();
        let first = take_frame_workload();
        assert!(first.draws >= 2);
        assert!(first.vertices >= 9);
        assert!(first.texture_uploads >= 1);
        assert_eq!(take_frame_workload(), FrameWorkload::default());
    }
}
