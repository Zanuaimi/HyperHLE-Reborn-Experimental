/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES and EAGL.
//!
//! This module is specific to OpenGL ES's role as a part of the iPhone OS API
//! surface. See [crate::gles] for other uses and a discussion of the broader
//! topic.

mod draw_stats;
mod eagl;
mod gles_guest;
mod software_vao;

use touchHLE_gl_bindings::gles11::types::{GLenum, GLint, GLsizei, GLuint};

use crate::mem::ConstPtr;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/OpenGLES.framework/OpenGLES",
    aliases: &[],
    class_exports: &[eagl::CLASSES],
    constant_exports: &[eagl::CONSTANTS],
    function_exports: &[gles_guest::FUNCTIONS, eagl::FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    /// Current EAGLContext for each thread
    current_ctxs: std::collections::HashMap<crate::ThreadId, Option<crate::objc::id>>,
    /// `glGetString()` results cache, keyed by `(is_es2, matrix_palette,
    /// name)` so that an ES 2.0 context does not return the ES 1.1
    /// `OpenGL ES-CM 1.1` version string (Bad Piggies / Unity 3.5 checks the
    /// version string to decide which renderer code path to use — mismatching
    /// the API leaves Unity in a half-initialised ES 2.0 path that emits torn /
    /// overlapping frames).
    ///
    /// `matrix_palette` is part of the key because the ES 1.1 `GL_EXTENSIONS`
    /// string advertises `GL_OES_matrix_palette` only when the active backend
    /// can honour it (see `GLES::supports_matrix_palette`), and the backend can
    /// change between sessions if the renderer is recreated.
    strings_cache: std::collections::HashMap<(bool, bool, GLenum), ConstPtr<u8>>,
    /// Guest `GLsync` handles (`glFenceSync` / `glFenceSyncAPPLE`). Host
    /// `GLsync` values are pointers and don't fit in a 32-bit guest value,
    /// so the guest gets an opaque handle into this table instead.
    sync_objects: std::collections::HashMap<GLuint, gles_guest::GuestSyncObject>,
    /// Next candidate guest `GLsync` handle (0 is never handed out).
    next_sync_handle: GLuint,
    /// The framebuffer object the guest currently has bound, per thread.
    bound_framebuffers: std::collections::HashMap<crate::ThreadId, GLuint>,
    /// Framebuffer objects whose colour attachment is a *texture*.
    ///
    /// The scale hack enlarges renderbuffers (both the EAGL drawable's and any
    /// the app allocates via `glRenderbufferStorage`), so scaling a viewport
    /// aimed at those is correct. Textures are never enlarged, so a viewport
    /// aimed at a texture-backed framebuffer must be left alone — otherwise the
    /// app draws `scale_hack` times too large into an unscaled target and you
    /// see a magnified bottom-left corner (Flappy Bird renders through a
    /// 288x512 texture FBO and did exactly that).
    texture_backed_framebuffers: std::collections::HashSet<GLuint>,
    /// The last viewport the guest asked for, in guest (unscaled) co-ordinates.
    ///
    /// The GL viewport is global state rather than per-framebuffer, and apps
    /// are free to call `glViewport` *before* binding the framebuffer they mean
    /// it for (Flappy Bird does). Whether the scale hack applies therefore
    /// cannot be decided when `glViewport` is called — we remember the request
    /// and re-apply it with the right scaling whenever the binding changes.
    guest_viewports: std::collections::HashMap<crate::ThreadId, (GLint, GLint, GLsizei, GLsizei)>,
    /// Whether the guest has `GL_DEPTH_TEST` enabled, per thread.
    ///
    /// The host's own `GL_DEPTH_TEST` bit is not always the guest's, because
    /// `depthless_framebuffers` below can force it off; this is the guest's
    /// request, which is what `glIsEnabled(GL_DEPTH_TEST)` must report.
    guest_depth_test: std::collections::HashMap<crate::ThreadId, bool>,
    /// Framebuffers whose only depth attachment is the one touchHLE
    /// synthesised for them.
    ///
    /// OpenGL ES says a framebuffer with no depth attachment behaves as if
    /// depth testing were disabled (Khronos OpenGL Wiki, "Depth Test":
    /// "When rendering to a framebuffer that has no depth buffer, depth testing
    /// always behaves as though the test is disabled"). So for these
    /// framebuffers `GL_DEPTH_TEST` is forced off on the host while they are
    /// bound. The synthesised buffer exists only to stop strict drivers from
    /// rejecting every fragment; letting it reject fragments of its own — from
    /// contents that start out undefined and that a guest which never attached
    /// depth has no reason to clear — turns "no depth buffer" into "depth
    /// buffer full of junk", which is the x-ray/black-scene shape.
    depthless_framebuffers: std::collections::HashSet<GLuint>,
}
impl State {
    fn current_ctx_for_thread(&mut self, thread: crate::ThreadId) -> &mut Option<crate::objc::id> {
        // PERF: this runs on every guest GL call; one hash lookup, not two.
        self.current_ctxs.entry(thread).or_insert(None)
    }

    pub fn set_bound_framebuffer(&mut self, thread: crate::ThreadId, framebuffer: GLuint) {
        self.bound_framebuffers.insert(thread, framebuffer);
    }

    /// Record what kind of colour attachment a framebuffer was given, so
    /// `glViewport` can tell whether the scale hack applies to it.
    pub fn set_colour_attachment_is_texture(&mut self, thread: crate::ThreadId, is_texture: bool) {
        let framebuffer = self
            .bound_framebuffers
            .get(&thread)
            .copied()
            .unwrap_or_default();
        // Framebuffer 0 is the window/drawable itself and is always scaled.
        if framebuffer == 0 {
            return;
        }
        if is_texture {
            self.texture_backed_framebuffers.insert(framebuffer);
        } else {
            self.texture_backed_framebuffers.remove(&framebuffer);
        }
    }

    /// Whether the scale hack should be applied to `glViewport` right now.
    ///
    /// Defaults to `true` (the historical behaviour) unless we positively know
    /// the bound framebuffer is texture-backed, so anything we failed to track
    /// keeps working exactly as before.
    pub fn set_guest_viewport(
        &mut self,
        thread: crate::ThreadId,
        viewport: (GLint, GLint, GLsizei, GLsizei),
    ) {
        self.guest_viewports.insert(thread, viewport);
    }

    pub fn guest_viewport(
        &self,
        thread: crate::ThreadId,
    ) -> Option<(GLint, GLint, GLsizei, GLsizei)> {
        self.guest_viewports.get(&thread).copied()
    }

    /// Record the guest's own `GL_DEPTH_TEST` request for a thread.
    pub fn set_guest_depth_test(&mut self, thread: crate::ThreadId, enabled: bool) {
        self.guest_depth_test.insert(thread, enabled);
    }

    /// The guest's own `GL_DEPTH_TEST` request for a thread.
    ///
    /// Depth testing is disabled in the initial GL state.
    pub fn guest_depth_test(&self, thread: crate::ThreadId) -> bool {
        self.guest_depth_test.get(&thread).copied().unwrap_or(false)
    }

    /// Record whether a framebuffer's depth attachment is touchHLE's, i.e.
    /// whether the guest itself never attached one.
    pub fn set_framebuffer_depthless(&mut self, framebuffer: GLuint, depthless: bool) {
        // Framebuffer 0 is the window/drawable itself; it has a depth buffer of
        // its own and this bookkeeping does not apply to it.
        if framebuffer == 0 {
            return;
        }
        if depthless {
            self.depthless_framebuffers.insert(framebuffer);
        } else {
            self.depthless_framebuffers.remove(&framebuffer);
        }
    }

    /// The framebuffer the guest has bound on this thread (0 is the window).
    pub fn bound_framebuffer(&self, thread: crate::ThreadId) -> GLuint {
        self.bound_framebuffers
            .get(&thread)
            .copied()
            .unwrap_or_default()
    }

    /// Whether the framebuffer bound by `thread` has no depth attachment of the
    /// guest's own, so depth testing must behave as disabled.
    pub fn bound_framebuffer_is_depthless(&self, thread: crate::ThreadId) -> bool {
        let framebuffer = self.bound_framebuffer(thread);
        framebuffer != 0 && self.depthless_framebuffers.contains(&framebuffer)
    }

    /// Whether `GL_DEPTH_TEST` should be enabled on the host right now: the
    /// guest wants it, and the bound framebuffer actually has a depth buffer
    /// the guest attached.
    pub fn host_depth_test(&self, thread: crate::ThreadId) -> bool {
        self.guest_depth_test(thread) && !self.bound_framebuffer_is_depthless(thread)
    }

    pub fn scale_hack_applies_to_viewport(&self, thread: crate::ThreadId) -> bool {
        let framebuffer = self
            .bound_framebuffers
            .get(&thread)
            .copied()
            .unwrap_or_default();
        !self.texture_backed_framebuffers.contains(&framebuffer)
    }
}

/// Bind the calling thread's current EAGL context to the host window and
/// return a [`crate::gles::GLES`] wrapper for it.
///
/// Returns `None` when the calling thread has no current EAGL context (i.e.
/// `[EAGLContext setCurrentContext:]` was never called with a non-`nil`
/// argument on this thread, or it was reset to `nil`). Apple's
/// implementation simply ignores GL calls in that state instead of crashing
/// the process — see Apple's "OpenGL ES Programming Guide for iOS",
/// section "Context Lifecycle Management": *"If the current context is
/// `nil`, OpenGL ES commands silently fail"*.
///
/// Previously this used `.unwrap()` on the thread-local context, which
/// turned a perfectly recoverable guest-side state error into an emulator
/// panic. HyperHLE log #5 reproduced this when a worker thread issued GL
/// calls between `[EAGLContext setCurrentContext:nil]` and a later
/// `setCurrentContext:` on the main thread.
fn sync_context<'objc, 'win: 'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    window: &'win mut crate::window::Window,
    current_thread: crate::ThreadId,
) -> Option<Box<dyn crate::gles::GLES + 'objc>> {
    let gles_ctx = get_thread_context(state, objc, current_thread)?;
    Some(gles_ctx.make_current(window))
}

/// Look up the current [`crate::gles::GLESContext`] for the given thread.
///
/// Returns `None` when the thread has no current EAGL context bound, or
/// when that context was created without a backing GLES driver (e.g.
/// headless / unsupported `EAGLRenderingAPI`). Both are recoverable —
/// callers should skip the GL operation rather than abort the emulator.
fn get_thread_context<'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    current_thread: crate::ThreadId,
) -> Option<&'objc mut dyn crate::gles::GLESContext> {
    // Note: `Box<dyn GLESContext>` carries the default `'static` trait-object
    // bound, so the returned mutable reference re-borrows from the
    // (`'objc`) ObjC store without imposing a tighter lifetime.
    let current_ctx = (*state.current_ctx_for_thread(current_thread))?;
    let host_obj = objc.borrow_mut::<eagl::EAGLContextHostObject>(current_ctx);
    let gles_ctx: &mut (dyn crate::gles::GLESContext + 'static) =
        host_obj.gles_ctx.as_deref_mut()?;
    Some(gles_ctx as &mut dyn crate::gles::GLESContext)
}

#[cfg(test)]
mod tests {
    use super::State;

    /// A framebuffer whose depth attachment touchHLE synthesised must behave as
    /// if depth testing were disabled, while the guest's own request is left
    /// alone so `glIsEnabled(GL_DEPTH_TEST)` can report it.
    ///
    /// OpenGL ES: "When rendering to a framebuffer that has no depth buffer,
    /// depth testing always behaves as though the test is disabled."
    /// (Khronos OpenGL Wiki, Depth Test.)
    #[test]
    fn depth_test_is_disabled_for_framebuffers_with_a_synthesised_depth_buffer() {
        let thread = 1;
        let mut state = State::default();
        state.set_guest_depth_test(thread, true);
        // Nothing bound, or the window drawable: the guest's request stands.
        assert!(state.host_depth_test(thread));
        state.set_bound_framebuffer(thread, 7);
        assert!(state.host_depth_test(thread));
        // The guest never attached a depth buffer to framebuffer 7.
        state.set_framebuffer_depthless(7, true);
        assert!(state.bound_framebuffer_is_depthless(thread));
        assert!(!state.host_depth_test(thread));
        assert!(state.guest_depth_test(thread));
        // Another framebuffer's depth-ness is not this one's.
        state.set_framebuffer_depthless(8, true);
        assert!(!state.host_depth_test(thread));
        // Binding a framebuffer that has a depth buffer of its own restores it.
        state.set_bound_framebuffer(thread, 9);
        assert!(state.host_depth_test(thread));
        // As does the guest attaching its own depth buffer to framebuffer 7.
        state.set_bound_framebuffer(thread, 7);
        assert!(!state.host_depth_test(thread));
        state.set_framebuffer_depthless(7, false);
        assert!(state.host_depth_test(thread));
    }

    /// The window drawable always has a depth buffer, so it is never treated as
    /// depth-less however the guest's own framebuffers are recorded.
    #[test]
    fn window_framebuffer_is_never_treated_as_depthless() {
        let thread = 1;
        let mut state = State::default();
        state.set_guest_depth_test(thread, true);
        state.set_framebuffer_depthless(0, true);
        state.set_bound_framebuffer(thread, 0);
        assert!(!state.bound_framebuffer_is_depthless(thread));
        assert!(state.host_depth_test(thread));
    }
}
