/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIWebView`.

use crate::android_web_view;
use crate::frameworks::core_graphics::{cg_image, CGRect};
use crate::frameworks::foundation::ns_dictionary::dict_from_keys_and_objects;
use crate::frameworks::foundation::ns_error::NSURLErrorDomain;
use crate::frameworks::foundation::ns_string::{self, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::image::Image;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, Class, ClassExports, NSZonePtr,
};
use crate::Environment;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// UIWebViewNavigationType constants
pub type UIWebViewNavigationType = i32;
pub const UIWebViewNavigationTypeLinkClicked: UIWebViewNavigationType = 0;
pub const UIWebViewNavigationTypeFormSubmitted: UIWebViewNavigationType = 1;
pub const UIWebViewNavigationTypeBackForward: UIWebViewNavigationType = 2;
pub const UIWebViewNavigationTypeReload: UIWebViewNavigationType = 3;
pub const UIWebViewNavigationTypeFormResubmitted: UIWebViewNavigationType = 4;
pub const UIWebViewNavigationTypeOther: UIWebViewNavigationType = 5;

// UIDataDetectorTypes bitmask
pub type UIDataDetectorTypes = NSUInteger;
pub const UIDataDetectorTypePhoneNumber: UIDataDetectorTypes = 1 << 0;
pub const UIDataDetectorTypeLink: UIDataDetectorTypes = 1 << 1;
pub const UIDataDetectorTypeAddress: UIDataDetectorTypes = 1 << 2;
pub const UIDataDetectorTypeCalendarEvent: UIDataDetectorTypes = 1 << 3;
pub const UIDataDetectorTypeNone: UIDataDetectorTypes = 0;
pub const UIDataDetectorTypeAll: UIDataDetectorTypes = u32::MAX as UIDataDetectorTypes;

/// A load that was initiated while the view had no on-screen extent (or
/// the host window wasn't ready), so the native overlay could not be
/// created yet. The load is remembered and retried from `-setFrame:`
/// once the view is laid out. See [retry_pending_load].
#[derive(Clone)]
enum PendingLoad {
    Url(String),
    /// (payload, MIME type, base URL)
    Data(String, String, String),
}

#[derive(Default)]
struct UIWebViewHostObject {
    superclass: UIViewHostObject,
    /// UIWebViewDelegate — weak reference (no retain per Apple docs)
    delegate: id,
    scales_page_to_fit: bool,
    detects_phone_numbers: bool,
    data_detector_types: UIDataDetectorTypes,
    allows_inline_media_playback: bool,
    media_playback_requires_user_action: bool,
    media_playback_allows_air_play: bool,
    suppress_incremental_rendering: bool,
    keyboard_display_requires_user_action: bool,
    pagination_mode: i32,
    pagination_breaking_mode: i32,
    page_length: f64,
    gap_between_pages: f64,
    /// NSString* — last URL string passed to loadRequest:
    current_url: id,

    loading: bool,
    load_id: i32,
    native_page_id: i32,
    last_deferred_retry: Option<Instant>,
    /// Simple back/forward stack — NSString* items.
    back_stack: Vec<id>,
    forward_stack: Vec<id>,
    /// Host-side native overlay (Android WebView) id. `-2` = no overlay has
    /// been created yet (first `show` seeds it with a page); `-1` = a show
    /// was attempted but unavailable/zero-size; `>= 0` = live overlay id.
    overlay_id: i32,
    /// Load deferred until the view has an on-screen extent.
    pending_load: Option<PendingLoad>,
    /// How many times the deferred load has been retried by the run loop so
    /// far (bounded so a view that never gets a frame can't poll forever).
    deferred_polls: u32,
}
impl_HostObject_with_superclass!(UIWebViewHostObject);

const DEFERRED_RETRY_INTERVAL: Duration = Duration::from_millis(600);
const MAX_DEFERRED_RETRIES: u32 = 100;
const NSURLErrorCancelled: i32 = -999;
const NSURLErrorTimedOut: i32 = -1001;
const NSURLErrorNotConnectedToInternet: i32 = -1009;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebViewEventAction {
    Started,
    Finished,
    Failed,
    Ignore,
}

fn webview_event_action(
    event_type: android_web_view::WebViewEventType,
    event_load_id: i32,
    event_page_id: i32,
    current_load_id: i32,
    current_page_id: i32,
    loading: bool,
) -> WebViewEventAction {
    if event_load_id != current_load_id {
        return WebViewEventAction::Ignore;
    }
    match event_type {
        android_web_view::WebViewEventType::Started if event_page_id > current_page_id => {
            WebViewEventAction::Started
        }
        android_web_view::WebViewEventType::Finished
            if loading && (current_page_id == 0 || event_page_id == current_page_id) =>
        {
            WebViewEventAction::Finished
        }
        android_web_view::WebViewEventType::Error
            if loading && (current_page_id == 0 || event_page_id == current_page_id) =>
        {
            WebViewEventAction::Failed
        }
        _ => WebViewEventAction::Ignore,
    }
}

fn make_webview_error(
    env: &mut Environment,
    domain: &str,
    code: i32,
    description: &str,
    url: &str,
) -> id {
    let domain = ns_string::from_rust_string(env, domain.to_owned());
    let description = ns_string::from_rust_string(env, description.to_owned());
    let url = ns_string::from_rust_string(env, url.to_owned());
    let description_key = ns_string::get_static_str(env, "NSLocalizedDescription");
    let url_key = ns_string::get_static_str(env, "NSErrorFailingURLStringKey");
    let user_info =
        dict_from_keys_and_objects(env, &[(description_key, description), (url_key, url)]);
    msg_class![env; NSError errorWithDomain:domain code:code userInfo:user_info]
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIWebView: UIView

// =========================================================================
// MARK: - Allocation
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIWebViewHostObject {
        superclass: UIViewHostObject::default(),
        delegate: nil,
        scales_page_to_fit: false,
        detects_phone_numbers: true,
        data_detector_types: UIDataDetectorTypePhoneNumber,
        allows_inline_media_playback: false,
        media_playback_requires_user_action: true,
        media_playback_allows_air_play: true,
        suppress_incremental_rendering: false,
        keyboard_display_requires_user_action: true,

        pagination_mode: 0,
        pagination_breaking_mode: 0,
        page_length: 0.0,
        gap_between_pages: 0.0,
        current_url: nil,
        loading: false,
        load_id: 0,
        native_page_id: 0,
        last_deferred_retry: None,
        back_stack: Vec::new(),
        forward_stack: Vec::new(),
        overlay_id: -2,
        pending_load: None,
        deferred_polls: 0,
    });
    let web_view = env.objc.alloc_object(this, host_object, &mut env.mem);
    env.framework_state.uikit.ui_view.web_views.push(web_view);
    web_view
}

// =========================================================================
// MARK: - Initializers
// =========================================================================

- (id)init {
    this
}

- (id)initWithFrame:(CGRect)_frame {
    this
}

- (id)initWithCoder:(id)_coder {
    this
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let host = env.objc.borrow::<UIWebViewHostObject>(this);
    let (current_url, back_stack, forward_stack, overlay_id) = (
        host.current_url,
        host.back_stack.clone(),
        host.forward_stack.clone(),
        host.overlay_id,
    );
    if overlay_id >= 0 {
        android_web_view::hide(overlay_id);
    }
    release(env, current_url);
    for url in back_stack    { release(env, url); }
    for url in forward_stack { release(env, url); }
    env.framework_state.uikit.ui_view.web_views.retain(|&view| view != this);
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - Delegate
// =========================================================================

- (id)delegate {
    env.objc.borrow::<UIWebViewHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // Weak reference — do NOT retain.
    env.objc.borrow_mut::<UIWebViewHostObject>(this).delegate = delegate;
}

// =========================================================================
// MARK: - Loading
// =========================================================================

- (())loadRequest:(id)request { // NSURLRequest*
    let url_string: String = if request != nil {
        let url: id = msg![env; request URL];
        let url_desc: id = msg![env; url description];
        if url_desc != nil { to_rust_string(env, url_desc).into_owned() } else { String::new() }
    } else {
        String::new()
    };
    log!("UIWebView loadRequest: {}", url_string);
    cancel_current_load(env, this, "A new request replaced the current WebView load.");

    // Push current URL onto back stack before navigating.
    let old_url = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if old_url != nil {
        retain(env, old_url);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).back_stack.push(old_url);
        // Clear forward stack on new navigation.
        let fwd: Vec<id> = std::mem::take(
            &mut env.objc.borrow_mut::<UIWebViewHostObject>(this).forward_stack
        );
        for u in fwd { release(env, u); }
    }
    release(env, old_url);
    let ns_url = ns_string::from_rust_string(env, url_string.clone());
    {
        env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = ns_url;
    }
    let load_id = begin_load(env, this);
    fire_did_start_load(env, this);

    let network_access = env.options.network_access;
    if !network_access_allows_url(&url_string, network_access) {
        let description = format!(
            "Network access is disabled. Enable Network access to load {}.",
            url_string
        );
        let error = make_webview_error(
            env,
            NSURLErrorDomain,
            NSURLErrorNotConnectedToInternet,
            &description,
            &url_string,
        );
        log!("UIWebView: blocked network request to {}", url_string);
        fail_load(env, this, error);
        return;
    }

    // Native completion/error callbacks arrive from MainActivity through the
    // Android event queue; never report finish merely because loadUrl returned.
    if android_web_view::native_webview_available() {
        if overlay_load(env, this, Some(&url_string), None, load_id) {
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        // Remember the load and retry it from -setFrame: once the view
        // is laid out.
        log!(
            "UIWebView: deferring load of {} until the view is laid out ({})",
            url_string,
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Url(url_string));
            host_obj.deferred_polls = 0;
            host_obj.last_deferred_retry = Some(Instant::now());
        }
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: snapshot the URL with headless Chromium and install
    // the PNG as this view's layer contents. We don't stream content, so the
    // load is "finished" as soon as the snapshot is in.
    let frame: CGRect = msg![env; this frame];
    let rendered = render_url_to_layer(env, this, &url_string, frame);
    finish_fallback_load(env, this, &url_string, rendered);
}

- (())loadHTMLString:(id)html     // NSString*
            baseURL:(id)_base_url { // NSURL*
    let html_str = if html != nil {
        to_rust_string(env, html).into_owned()
    } else { String::new() };
    let base_url = if _base_url != nil {
        let url: id = msg![env; _base_url description];
        if url != nil { to_rust_string(env, url).into_owned() } else { String::new() }
    } else {
        String::new()
    };
    log_dbg!("UIWebView loadHTMLString: ({} chars)", html_str.len());
    cancel_current_load(env, this, "A new request replaced the current WebView load.");
    let load_id = begin_load(env, this);
    fire_did_start_load(env, this);

    if android_web_view::native_webview_available() {
        if overlay_load(env, this, None, Some((&html_str, "text/html", &base_url)), load_id) {
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        log!(
            "UIWebView: deferring HTML load until the view is laid out ({})",
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Data(
                html_str,
                "text/html".to_string(),
                base_url,
            ));
            host_obj.deferred_polls = 0;
            host_obj.last_deferred_retry = Some(Instant::now());
        }
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: write the HTML to a temp file and snapshot it.
    let frame: CGRect = msg![env; this frame];
    let rendered = render_html_to_layer(env, this, &html_str, frame);
    finish_fallback_load(env, this, "loadHTMLString", rendered);
}

- (())loadData:(id)data            // NSData*
      MIMEType:(id)mime            // NSString*
      textEncodingName:(id)_enc    // NSString*
       baseURL:(id)_base_url {     // NSURL*
    let mime_str = if mime != nil { to_rust_string(env, mime).into_owned() } else { "(null)".into() };
    let base_url = if _base_url != nil {
        let url: id = msg![env; _base_url description];
        if url != nil { to_rust_string(env, url).into_owned() } else { String::new() }
    } else {
        String::new()
    };
    log_dbg!("UIWebView loadData:MIMEType:{}", mime_str);
    cancel_current_load(env, this, "A new request replaced the current WebView load.");
    let load_id = begin_load(env, this);
    fire_did_start_load(env, this);

    // Read the payload out of the guest NSData.
    let payload = if data != nil {
        let len: NSUInteger = msg![env; data length];
        let bytes_ptr: crate::mem::ConstPtr<u8> = msg![env; data bytes];
        if len > 0 && !bytes_ptr.is_null() {
            let bytes = env.mem.bytes_at(bytes_ptr, len);
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    if android_web_view::native_webview_available() {
        if overlay_load(
            env,
            this,
            None,
            Some((&payload, mime_str.as_str(), &base_url)),
            load_id,
        ) {
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        log!(
            "UIWebView: deferring data load until the view is laid out ({})",
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Data(payload, mime_str, base_url));
            host_obj.deferred_polls = 0;
            host_obj.last_deferred_retry = Some(Instant::now());
        }
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: only HTML payloads can be rendered (temp file route).
    let frame: CGRect = msg![env; this frame];
    let rendered = mime_str.starts_with("text/html")
        && render_html_to_layer(env, this, &payload, frame);
    finish_fallback_load(env, this, "loadData", rendered);
}

// =========================================================================
// MARK: - Subview management
// =========================================================================

- (())insertSubview:(id)view aboveSubview:(id)sibling {
    if view == nil { return; }

    // If sibling is nil or not in our view hierarchy, just add at the top.
    if sibling == nil {
        let _: () = msg![env; this addSubview:view];
        return;
    }

    // Delegate to UIView's insertSubview:aboveSubview: on our own view.
    let self_view: id = msg![env; this view];
    if self_view != nil {
        let _: () = msg![env; self_view insertSubview:view aboveSubview:sibling];
    } else {
        // Fallback — just add it.
        let _: () = msg![env; this addSubview:view];
    }
}

- (())reload {
    log_dbg!("UIWebView reload");
    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current == nil { return; }
    retain(env, current);

    // Вынесено в отдельную переменную для предотвращения ошибки E0283
    let url: id = msg_class![env; NSURL URLWithString:current];
    let ns_req: id = msg_class![env; NSURLRequest requestWithURL:url];

    let _: () = msg![env; this loadRequest:ns_req];
    release(env, current);
}

- (())stopLoading {
    log_dbg!("UIWebView stopLoading");
    cancel_current_load(env, this, "The WebView load was cancelled.");
}

- (bool)isLoading {
    env.objc.borrow::<UIWebViewHostObject>(this).loading
}

// =========================================================================
// MARK: - Navigation
// =========================================================================

- (bool)canGoBack {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_back(overlay_id) {
        return true;
    }
    !env.objc.borrow::<UIWebViewHostObject>(this).back_stack.is_empty()
}

- (bool)canGoForward {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_forward(overlay_id) {
        return true;
    }
    !env.objc.borrow::<UIWebViewHostObject>(this).forward_stack.is_empty()
}

- (())goBack {
    let back_url = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        host.back_stack.pop()
    };
    let Some(back_url) = back_url else { return; };
    cancel_current_load(env, this, "A back navigation replaced the current WebView load.");

    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current != nil {
        retain(env, current);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).forward_stack.push(current);
    }
    release(env, current);
    env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = back_url;
    let url = to_rust_string(env, back_url).into_owned();
    let load_id = begin_load(env, this);
    fire_did_start_load(env, this);

    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_back(overlay_id) {
        android_web_view::go_back(overlay_id, load_id);
    } else if android_web_view::native_webview_available() {
        if !overlay_load(env, this, Some(&url), None, load_id) {
            defer_url_load(env, this, url);
        }
    } else {
        let frame: CGRect = msg![env; this frame];
        let rendered = render_url_to_layer(env, this, &url, frame);
        finish_fallback_load(env, this, &url, rendered);
    }
}

- (())goForward {
    let fwd_url = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        host.forward_stack.pop()
    };
    let Some(fwd_url) = fwd_url else { return; };
    cancel_current_load(env, this, "A forward navigation replaced the current WebView load.");

    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current != nil {
        retain(env, current);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).back_stack.push(current);
    }
    release(env, current);
    env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = fwd_url;
    let url = to_rust_string(env, fwd_url).into_owned();
    let load_id = begin_load(env, this);
    fire_did_start_load(env, this);

    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_forward(overlay_id) {
        android_web_view::go_forward(overlay_id, load_id);
    } else if android_web_view::native_webview_available() {
        if !overlay_load(env, this, Some(&url), None, load_id) {
            defer_url_load(env, this, url);
        }
    } else {
        let frame: CGRect = msg![env; this frame];
        let rendered = render_url_to_layer(env, this, &url, frame);
        finish_fallback_load(env, this, &url, rendered);
    }
}

// =========================================================================
// MARK: - JavaScript
// =========================================================================

- (id)stringByEvaluatingJavaScriptFromString:(id)script { // NSString* -> NSString*
    let script_str = if script != nil {
        to_rust_string(env, script).into_owned()
    } else { String::new() };
    log_dbg!("UIWebView stringByEvaluatingJavaScriptFromString: {:?}", script_str);

    // Real path: evaluate in the native Android WebView and return its
    // result. Android returns the value JSON-encoded; unwrap plain strings
    // and nulls so guests see something close to UIWebView's conversion.
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        if let Some(result) = android_web_view::eval_js(overlay_id, &script_str) {
            let unwrapped = unwrap_js_result(&result);
            let s = ns_string::from_rust_string(env, unwrapped);
            return crate::objc::autorelease(env, s);
        }
    }
    // Return empty NSString rather than nil — some apps check the return value.
    let empty = ns_string::from_rust_string(env, String::new());
    crate::objc::autorelease(env, empty)
}

// =========================================================================
// MARK: - Request / URL accessors
// =========================================================================

- (id)request { // NSURLRequest*
    let url = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if url == nil {
        return nil;
    }
    let ns_url: id = msg_class![env; NSURL URLWithString:url];
    if ns_url == nil {
        return nil;
    }
    msg_class![env; NSURLRequest requestWithURL:ns_url]
}

// Returns the URL of the currently loaded page as an NSString*.
- (id)_currentURLString { // NSString* (private helper)
    env.objc.borrow::<UIWebViewHostObject>(this).current_url
}

// =========================================================================
// MARK: - Properties
// =========================================================================

- (bool)scalesPageToFit {
    env.objc.borrow::<UIWebViewHostObject>(this).scales_page_to_fit
}
- (())setScalesPageToFit:(bool)scales {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).scales_page_to_fit = scales;
}

- (bool)detectsPhoneNumbers {
    env.objc.borrow::<UIWebViewHostObject>(this).detects_phone_numbers
}
- (())setDetectsPhoneNumbers:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).detects_phone_numbers = value;
}

- (UIDataDetectorTypes)dataDetectorTypes {
    env.objc.borrow::<UIWebViewHostObject>(this).data_detector_types
}
- (())setDataDetectorTypes:(UIDataDetectorTypes)types {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).data_detector_types = types;
}

- (bool)allowsInlineMediaPlayback {
    env.objc.borrow::<UIWebViewHostObject>(this).allows_inline_media_playback
}
- (())setAllowsInlineMediaPlayback:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).allows_inline_media_playback = value;
}

- (bool)mediaPlaybackRequiresUserAction {
    env.objc.borrow::<UIWebViewHostObject>(this).media_playback_requires_user_action
}
- (())setMediaPlaybackRequiresUserAction:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).media_playback_requires_user_action = value;
}

- (bool)mediaPlaybackAllowsAirPlay {
    env.objc.borrow::<UIWebViewHostObject>(this).media_playback_allows_air_play
}
- (())setMediaPlaybackAllowsAirPlay:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).media_playback_allows_air_play = value;
}

- (bool)suppressesIncrementalRendering {
    env.objc.borrow::<UIWebViewHostObject>(this).suppress_incremental_rendering
}
- (())setSuppressesIncrementalRendering:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).suppress_incremental_rendering = value;
}

- (bool)keyboardDisplayRequiresUserAction {
    env.objc.borrow::<UIWebViewHostObject>(this).keyboard_display_requires_user_action
}
- (())setKeyboardDisplayRequiresUserAction:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).keyboard_display_requires_user_action = value;
}

// Pagination (iOS 7+)
- (i32)paginationMode { // UIWebPaginationMode
    env.objc.borrow::<UIWebViewHostObject>(this).pagination_mode
}
- (())setPaginationMode:(i32)mode {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).pagination_mode = mode;
}

- (i32)paginationBreakingMode { // UIWebPaginationBreakingMode
    env.objc.borrow::<UIWebViewHostObject>(this).pagination_breaking_mode
}
- (())setPaginationBreakingMode:(i32)mode {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).pagination_breaking_mode = mode;
}

- (f64)pageLength {
    env.objc.borrow::<UIWebViewHostObject>(this).page_length
}
- (())setPageLength:(f64)length {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).page_length = length;
}

- (f64)gapBetweenPages {
    env.objc.borrow::<UIWebViewHostObject>(this).gap_between_pages
}
- (())setGapBetweenPages:(f64)gap {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).gap_between_pages = gap;
}

- (u32)pageCount { // NSUInteger
    // No real rendering — always 0.
    0u32
}

// =========================================================================
// MARK: - Layout
// =========================================================================

- (())setFrame:(CGRect)frame {
    // `() = ` pins the void return type (house style for non-tail super
    // calls, e.g. UITableView's own -setFrame: override).
    () = msg_super![env; this setFrame:frame];
    // A load may have been deferred because the view had no on-screen
    // extent when the app initiated it (apps commonly load the request
    // before the view is laid out). Now that the view has a size, take
    // the deferred load.
    retry_pending_load(env, this);
}

// =========================================================================
// MARK: - View hierarchy (overlay teardown)
// =========================================================================

- (())removeFromSuperview {
    cancel_current_load(env, this, "The WebView was removed while its load was in progress.");
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        android_web_view::hide(overlay_id);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).overlay_id = -2;
    }
    // Mirror UIView's removeFromSuperview: detach from the superview. The
    // layer detaches itself from the superlayer; the superview drops `this`
    // from its `subviews` (which balances the retain taken by addSubview).
    let superview = env
        .objc
        .borrow_mut::<UIWebViewHostObject>(this)
        .superclass
        .superview;
    if superview == nil {
        return;
    }
    let layer = env.objc.borrow::<UIWebViewHostObject>(this).superclass.layer;
    let _: () = msg![env; layer removeFromSuperlayer];
    let removed = {
        let subviews = &mut env
            .objc
            .borrow_mut::<UIViewHostObject>(superview)
            .subviews;
        if let Some(idx) = subviews.iter().position(|&v| v == this) {
            subviews.remove(idx);
            true
        } else {
            false
        }
    };
    if removed {
        release(env, this);
    }
}

// =========================================================================
// MARK: - Scroll view
// =========================================================================

// Returns a stub scroll view so apps that access scrollView don't crash.
- (id)scrollView { // UIScrollView*
    // Return self as a passthrough — we don't have a real UIScrollView here.
    this
}

// =========================================================================
// MARK: - Native-overlay & async-load internals
// =========================================================================



// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let (loading, current_url) = {
        let h = env.objc.borrow::<UIWebViewHostObject>(this);
        (h.loading, h.current_url)
    };
    let url_str = if current_url != nil {
        to_rust_string(env, current_url).into_owned()
    } else { "(nil)".into() };
    let s = format!(
        "<UIWebView: {:?}; loading={}; url={}>",
        this, loading, url_str
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

// =========================================================================
// MARK: - Chromium/CDP bridge: render a URL into the view's layer.contents
// =========================================================================
//
// touchHLE has no HTML rendering engine. As an opportunistic fallback (see
// PR description) we shell out to the host's headless Chromium to rasterise
// the target URL into a PNG, then install that PNG as the CALayer contents
// for the UIWebView. This gives apps like Google Mobile a visible web page
// instead of a blank rectangle, at the cost of interactivity.

/// Counter used for unique temp filenames.
static SNAP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Find a headless-capable Chromium binary on the host. Returns `None` when
/// no suitable browser is available — in that case we leave the layer blank.
fn find_chromium_binary() -> Option<PathBuf> {
    // Allow env var override for advanced users / CI.
    if let Ok(path) = std::env::var("TOUCHHLE_CHROMIUM") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    let candidates = [
        "/opt/.devin/chrome/chrome/linux-137.0.7118.2/chrome-linux64/chrome",
        "/opt/.devin/playwright_browsers/chromium-1097/chrome-linux/chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome-stable",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Shell out to headless Chromium to snapshot `url` at `width x height` and
/// return the PNG bytes, or `None` on failure. Callers report failures to the guest.
fn snapshot_url_with_chromium(
    url: &str,
    width: u32,
    height: u32,
    network_access: bool,
) -> Option<Vec<u8>> {
    let Some(chrome) = find_chromium_binary() else {
        log!("UIWebView bridge: no Chromium binary found; leaving layer blank");
        return None;
    };
    let idx = SNAP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("touchhle_uiwebview_{}.png", idx));
    // Chromium refuses to run as root unless given --no-sandbox.
    let mut command = Command::new(&chrome);
    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg(format!("--window-size={},{}", width, height))
        .arg(format!("--screenshot={}", tmp.display()))
        .arg(url);
    if !network_access {
        command.arg("--disable-networking");
    }
    let status = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            log!("UIWebView bridge: chromium exited with {}", s);
        }
        Err(e) => {
            log!("UIWebView bridge: failed to spawn chromium: {}", e);
            return None;
        }
    }
    let bytes = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    bytes
}

/// Snapshot `url` and install the decoded PNG as the UIWebView's
/// `layer.contents` so the user sees the rendered web page.
fn render_url_to_layer(env: &mut Environment, this: id, url: &str, frame: CGRect) -> bool {
    if cfg!(target_os = "android") {
        // There is no desktop Chromium to shell out to on a phone: the
        // native WebView overlay is the only real rendering path there.
        return false;
    }
    if url.is_empty()
        || !(url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("file://"))
    {
        return false;
    }
    let width = (frame.size.width.max(1.0) as u32).max(1);
    let height = (frame.size.height.max(1.0) as u32).max(1);
    let Some(png) = snapshot_url_with_chromium(url, width, height, env.options.network_access)
    else {
        return false;
    };
    let Ok(image) = Image::from_bytes(&png) else {
        log!("UIWebView bridge: could not decode PNG snapshot");
        return false;
    };
    let cg_image = cg_image::from_image(env, image);
    let layer: id = msg![env; this layer];
    let _: () = msg![env; layer setContents:cg_image];
    let _: () = msg![env; this setNeedsDisplay];
    cg_image::CGImageRelease(env, cg_image);
    true
}

fn render_html_to_layer(env: &mut Environment, this: id, html: &str, frame: CGRect) -> bool {
    if html.is_empty() {
        let layer: id = msg![env; this layer];
        () = msg![env; layer setContents:nil];
        () = msg![env; this setNeedsDisplay];
        return true;
    }
    let idx = SNAP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("touchhle_uiwebview_{}.html", idx));
    if std::fs::write(&tmp, html.as_bytes()).is_err() {
        return false;
    }
    let url = format!("file://{}", tmp.display());
    let rendered = render_url_to_layer(env, this, &url, frame);
    let _ = std::fs::remove_file(&tmp);
    rendered
}

fn finish_fallback_load(env: &mut Environment, this: id, url: &str, rendered: bool) {
    if rendered {
        finish_load(env, this);
    } else {
        let description = format!("Host could not render WebView content for {}.", url);
        let error = make_webview_error(env, "TouchHLEWebViewErrorDomain", 1, &description, url);
        fail_load(env, this, error);
    }
}

// =========================================================================
// MARK: - Native overlay plumbing
// =========================================================================

/// Map the webview's guest frame to host window pixels and create/position
/// the native overlay. Returns the overlay id, or `None` if the view has no
/// on-screen extent (or native overlays are unavailable).
fn overlay_show_or_update(env: &mut Environment, this: id) -> Option<i32> {
    if !android_web_view::native_webview_available() {
        return None;
    }
    let frame: CGRect = msg![env; this frame];
    let (mut x, mut y, mut w, mut h) = env.window().guest_frame_to_window_px(frame);
    if w <= 0 || h <= 0 {
        // The view itself has no extent yet. Apps commonly add the web
        // view to a container and let autoresizing size it — which never
        // happens here, as touchHLE has no automatic layout pass. If we
        // already have a superview with a usable frame, borrow its
        // geometry instead of staying blank.
        let superview: id = msg![env; this superview];
        if superview != nil {
            let super_frame: CGRect = msg![env; superview frame];
            let (sx, sy, sw, sh) = env.window().guest_frame_to_window_px(super_frame);
            if sw > 0 && sh > 0 {
                log!("UIWebView overlay: view has no extent yet; using superview frame");
                x = sx;
                y = sy;
                w = sw;
                h = sh;
            }
        }
    }
    if w <= 0 || h <= 0 {
        log!("UIWebView overlay skipped: view has no on-screen extent yet");
        return None;
    }
    let current = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if current == -2 {
        // First show: create the overlay seeded with a blank page; the
        // caller immediately navigates or loads data into it.
        let oid = android_web_view::show(x, y, w, h, env.options.network_access);
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        if oid >= 0 {
            host.overlay_id = oid;
            Some(oid)
        } else {
            host.overlay_id = -2;
            None
        }
    } else if current >= 0 {
        android_web_view::set_bounds(current, x, y, w, h);
        Some(current)
    } else {
        None
    }
}

/// Carry out a load that was deferred because the view had no on-screen
/// extent when the app initiated it. Called from `-setFrame:`; keeps the
/// load deferred if the view still can't be shown.
fn has_webview_extent(frame: (f32, f32), superview: Option<(f32, f32)>) -> bool {
    (frame.0 > 0.0 && frame.1 > 0.0) || superview.is_some_and(|size| size.0 > 0.0 && size.1 > 0.0)
}

fn network_access_allows_url(url: &str, network_access: bool) -> bool {
    android_web_view::network_access_allows_url(url, network_access)
}

fn defer_url_load(env: &mut Environment, this: id, url: String) {
    log!(
        "UIWebView: deferring URL load of {} ({})",
        url,
        deferred_load_context(env, this)
    );
    let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
    host.pending_load = Some(PendingLoad::Url(url));
    host.deferred_polls = 0;
    host.last_deferred_retry = Some(Instant::now());
}

fn retry_pending_load(env: &mut Environment, this: id) {
    if env
        .objc
        .borrow::<UIWebViewHostObject>(this)
        .pending_load
        .is_none()
    {
        return;
    }
    if !android_web_view::native_webview_available() {
        return;
    }
    let frame: CGRect = msg![env; this frame];
    if !has_webview_extent((frame.size.width, frame.size.height), None) {
        // The view itself has no size yet. The overlay can still be shown
        // from the superview's geometry (see overlay_show_or_update), so
        // only keep waiting when there is no usable superview either.
        let superview: id = msg![env; this superview];
        if superview == nil {
            return;
        }
        let super_frame: CGRect = msg![env; superview frame];
        if super_frame.size.width <= 0.0 || super_frame.size.height <= 0.0 {
            return;
        }
    }
    let pending = env
        .objc
        .borrow_mut::<UIWebViewHostObject>(this)
        .pending_load
        .take()
        .unwrap();
    log!("UIWebView: retrying deferred load now that the view is laid out");
    let shown = match &pending {
        PendingLoad::Url(url) => {
            let load_id = env.objc.borrow::<UIWebViewHostObject>(this).load_id;
            overlay_load(env, this, Some(url), None, load_id)
        }
        PendingLoad::Data(payload, mime, base_url) => {
            let load_id = env.objc.borrow::<UIWebViewHostObject>(this).load_id;
            overlay_load(env, this, None, Some((payload, mime, base_url)), load_id)
        }
    };
    if !shown {
        // The overlay still couldn't be shown (e.g. the host window
        // viewport isn't ready); keep the load deferred.
        env.objc
            .borrow_mut::<UIWebViewHostObject>(this)
            .pending_load = Some(pending);
    }
}

/// One-line description of a deferred webview's geometry: its own frame
/// and, if it has one, its superview's frame. Included in the deferral log
/// so that a single line shows whether the superview fallback (see
/// `overlay_show_or_update`) will be able to size the overlay — and so the
/// line itself identifies builds that carry this diagnostic.
pub(crate) fn handle_native_load_events(env: &mut Environment) -> (Option<Instant>, bool) {
    if !android_web_view::native_webview_available() {
        return (None, false);
    }
    let web_views = env.framework_state.uikit.ui_view.web_views.clone();
    if web_views.is_empty() {
        return (None, false);
    }

    let now = Instant::now();
    let mut handled = false;
    for &web_view in &web_views {
        let retry_due = {
            let host = env.objc.borrow::<UIWebViewHostObject>(web_view);
            host.pending_load.is_some()
                && host.last_deferred_retry.map_or(true, |last| {
                    now.duration_since(last) >= DEFERRED_RETRY_INTERVAL
                })
        };
        if !retry_due {
            continue;
        }

        handled = true;
        env.objc
            .borrow_mut::<UIWebViewHostObject>(web_view)
            .last_deferred_retry = Some(now);
        retry_pending_load(env, web_view);
        let (polls, current_url) = {
            let host = env.objc.borrow::<UIWebViewHostObject>(web_view);
            if host.pending_load.is_none() {
                continue;
            }
            (host.deferred_polls.saturating_add(1), host.current_url)
        };
        let url = if current_url != nil {
            to_rust_string(env, current_url).into_owned()
        } else {
            String::new()
        };
        env.objc
            .borrow_mut::<UIWebViewHostObject>(web_view)
            .deferred_polls = polls;
        if polls == 1 || polls % 10 == 0 {
            log!(
                "UIWebView: deferred load retry {}/{} ({})",
                polls,
                MAX_DEFERRED_RETRIES,
                deferred_load_context(env, web_view)
            );
        }
        if polls >= MAX_DEFERRED_RETRIES {
            let error = make_webview_error(
                env,
                NSURLErrorDomain,
                NSURLErrorTimedOut,
                "The WebView did not receive a usable frame before the layout timeout.",
                &url,
            );
            log!(
                "UIWebView: deferred load failed after {} retries ({})",
                polls,
                url
            );
            fail_load(env, web_view, error);
        }
    }

    while let Some(json) = android_web_view::poll_webview_event() {
        handled = true;
        let event = match android_web_view::parse_webview_event(&json) {
            Ok(event) => event,
            Err(error) => {
                log!(
                    "UIWebView: ignoring malformed Android WebView event: {}",
                    error
                );
                continue;
            }
        };
        let Some(&web_view) = web_views.iter().find(|&&view| {
            let host = env.objc.borrow::<UIWebViewHostObject>(view);
            host.overlay_id == event.overlay_id
        }) else {
            log!(
                "UIWebView: ignoring event for removed overlay {}",
                event.overlay_id
            );
            continue;
        };
        let action = {
            let host = env.objc.borrow::<UIWebViewHostObject>(web_view);
            webview_event_action(
                event.event_type,
                event.load_id,
                event.page_id,
                host.load_id,
                host.native_page_id,
                host.loading,
            )
        };
        if action == WebViewEventAction::Ignore {
            log!(
                "UIWebView: ignoring stale {:?} event for overlay {} load {}",
                event.event_type,
                event.overlay_id,
                event.load_id
            );
            continue;
        }

        match action {
            WebViewEventAction::Started => {
                let url = ns_string::from_rust_string(env, event.url);
                let (old_url, should_notify) = {
                    let host = env.objc.borrow_mut::<UIWebViewHostObject>(web_view);
                    host.native_page_id = event.page_id;
                    let should_notify = !host.loading;
                    host.loading = true;
                    (std::mem::replace(&mut host.current_url, url), should_notify)
                };
                release(env, old_url);
                if should_notify {
                    fire_did_start_load(env, web_view);
                }
            }
            WebViewEventAction::Finished => {
                log!("UIWebView: native page finished: {}", event.url);
                env.objc
                    .borrow_mut::<UIWebViewHostObject>(web_view)
                    .native_page_id = event.page_id;
                finish_load(env, web_view);
            }
            WebViewEventAction::Failed => {
                log!(
                    "UIWebView: native page failed: code={} url={} description={}",
                    event.error_code,
                    event.url,
                    event.description
                );
                env.objc
                    .borrow_mut::<UIWebViewHostObject>(web_view)
                    .native_page_id = event.page_id;
                let error = make_webview_error(
                    env,
                    "AndroidWebViewErrorDomain",
                    event.error_code,
                    &event.description,
                    &event.url,
                );
                fail_load(env, web_view, error);
            }
            WebViewEventAction::Ignore => unreachable!(),
        }
    }

    let active_load = web_views
        .iter()
        .any(|&view| env.objc.borrow::<UIWebViewHostObject>(view).loading);
    let next_poll = active_load.then(|| Instant::now() + Duration::from_millis(50));
    (next_poll, handled)
}

fn deferred_load_context(env: &mut Environment, this: id) -> String {
    let frame: CGRect = msg![env; this frame];
    // CGRect is #[repr(packed)]: references to its fields in format args
    // would be unaligned (E0793), so copy the values out first.
    let frame_w = frame.size.width;
    let frame_h = frame.size.height;
    let superview: id = msg![env; this superview];
    if superview == nil {
        return format!("frame {}x{}, no superview yet", frame_w, frame_h);
    }
    let super_frame: CGRect = msg![env; superview frame];
    let super_w = super_frame.size.width;
    let super_h = super_frame.size.height;
    format!(
        "frame {}x{}, superview {:?} {}x{}",
        frame_w, frame_h, superview, super_w, super_h
    )
}

/// Retry deferred loads for every UIWebView in `root`'s view subtree.
/// Called when a freshly built hierarchy has just been attached to a window
/// (see `-[UIViewController presentModalViewController:animated:]`): at that
/// point UIWebViews finally have a superview with a non-zero frame.
pub(crate) fn retry_pending_loads_in_subtree(env: &mut Environment, root: id) {
    if root == nil {
        return;
    }
    let uiwebview_class: id = msg_class![env; UIWebView class];
    let is_webview: bool = msg![env; root isKindOfClass:uiwebview_class];
    if is_webview {
        let has_pending = env
            .objc
            .borrow::<UIWebViewHostObject>(root)
            .pending_load
            .is_some();
        if has_pending {
            log!(
                "UIWebView: hierarchy attached; retrying deferred load ({})",
                deferred_load_context(env, root)
            );
            retry_pending_load(env, root);
        }
    }
    let subviews: id = msg![env; root subviews];
    if subviews != nil {
        let count: NSUInteger = msg![env; subviews count];
        let mut i: NSUInteger = 0;
        while i < count {
            let child: id = msg![env; subviews objectAtIndex:i];
            retry_pending_loads_in_subtree(env, child);
            i += 1;
        }
    }
}

fn begin_load(env: &mut Environment, this: id) -> android_web_view::WebViewLoadId {
    let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
    host.load_id = host.load_id.wrapping_add(1).max(1);
    host.loading = true;
    host.native_page_id = 0;
    host.pending_load = None;
    host.deferred_polls = 0;
    host.last_deferred_retry = None;
    host.load_id
}

fn fail_load(env: &mut Environment, this: id, error: id) {
    let was_loading = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        let was_loading = host.loading;
        host.loading = false;
        host.pending_load = None;
        host.last_deferred_retry = None;
        was_loading
    };
    if was_loading {
        fire_did_fail_load(env, this, error);
    }
}

fn cancel_current_load(env: &mut Environment, this: id, description: &str) {
    let (overlay_id, current_url) = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        if !host.loading {
            return;
        }
        host.load_id = host.load_id.wrapping_add(1).max(1);
        host.loading = false;
        host.pending_load = None;
        host.last_deferred_retry = None;
        (host.overlay_id, host.current_url)
    };
    let url = if current_url != nil {
        to_rust_string(env, current_url).into_owned()
    } else {
        String::new()
    };
    if overlay_id >= 0 {
        android_web_view::stop_loading(overlay_id);
    }
    let error = make_webview_error(
        env,
        NSURLErrorDomain,
        NSURLErrorCancelled,
        description,
        &url,
    );
    fire_did_fail_load(env, this, error);
}

/// Fire `webViewDidFailLoadWithError:` on the delegate (if it responds).
fn fire_did_fail_load(env: &mut Environment, this: id, error: id) {
    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env
            .objc
            .register_host_selector("webView:didFailLoadWithError:".to_string(), &mut env.mem);
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate webView:this didFailLoadWithError:error];
        }
    }
}

/// Fire `webViewDidFinishLoad:` on the delegate (if it responds).
fn fire_did_finish_load(env: &mut Environment, this: id) {
    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env
            .objc
            .register_host_selector("webViewDidFinishLoad:".to_string(), &mut env.mem);
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate webViewDidFinishLoad:this];
        }
    }
}

/// Fire `webViewDidStartLoad:` on the delegate (if it responds).
fn fire_did_start_load(env: &mut Environment, this: id) {
    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env
            .objc
            .register_host_selector("webViewDidStartLoad:".to_string(), &mut env.mem);
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate webViewDidStartLoad:this];
        }
    }
}

/// Show (or update) the native overlay and load `url` / `data` into it.
/// `url` is `Some` for URL loads; `data` is `(payload, mime)` for data loads.
/// Returns `true` when the native path was taken.
fn overlay_load(
    env: &mut Environment,
    this: id,
    url: Option<&str>,
    data: Option<(&str, &str, &str)>,
    load_id: android_web_view::WebViewLoadId,
) -> bool {
    let Some(overlay) = overlay_show_or_update(env, this) else {
        return false;
    };
    if let Some((payload, mime, base_url)) = data {
        android_web_view::load_data(overlay, payload, mime, base_url, load_id);
    } else if let Some(url) = url {
        if url.is_empty() {
            android_web_view::load_data(
                overlay,
                "<html><body></body></html>",
                "text/html",
                "",
                load_id,
            );
        } else {
            android_web_view::navigate(overlay, url, load_id);
        }
    }
    true
}

/// Mark the load as finished and fire `webViewDidFinishLoad:`.
fn finish_load(env: &mut Environment, this: id) {
    let was_loading = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        let was_loading = host.loading;
        host.loading = false;
        host.pending_load = None;
        host.last_deferred_retry = None;
        was_loading
    };
    if was_loading {
        fire_did_finish_load(env, this);
    }
}

/// Android's `evaluateJavascript` returns a JSON-encoded value; unwrap the
/// common cases (quoted string, null, bare literals) into a plain string.
fn unwrap_js_result(result: &str) -> String {
    let t = result.trim();
    if t == "null" {
        return String::new();
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        return inner
            .replace("\\\\", "\u{1}")
            .replace("\\n", "\n")
            .replace("\\r", "\r")
            .replace("\\t", "\t")
            .replace("\\\"", "\"")
            .replace('\u{1}', "\\");
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        has_webview_extent, network_access_allows_url, webview_event_action, WebViewEventAction,
    };
    use crate::android_web_view::WebViewEventType;

    #[test]
    fn native_webview_callbacks_require_current_load_and_page() {
        let current_load = 4;
        let page = 7;
        assert_eq!(
            webview_event_action(
                WebViewEventType::Started,
                current_load,
                page,
                current_load,
                0,
                true,
            ),
            WebViewEventAction::Started
        );
        assert_eq!(
            webview_event_action(
                WebViewEventType::Finished,
                current_load,
                page,
                current_load,
                page,
                true,
            ),
            WebViewEventAction::Finished
        );
        assert_eq!(
            webview_event_action(
                WebViewEventType::Error,
                current_load,
                page,
                current_load,
                page,
                true,
            ),
            WebViewEventAction::Failed
        );
        assert_eq!(
            webview_event_action(
                WebViewEventType::Finished,
                current_load - 1,
                page,
                current_load,
                page,
                true,
            ),
            WebViewEventAction::Ignore
        );
        assert_eq!(
            webview_event_action(
                WebViewEventType::Finished,
                current_load,
                page - 1,
                current_load,
                page,
                true,
            ),
            WebViewEventAction::Ignore
        );
        assert_eq!(
            webview_event_action(
                WebViewEventType::Finished,
                current_load,
                page,
                current_load,
                page,
                false,
            ),
            WebViewEventAction::Ignore
        );
    }

    #[test]
    fn network_access_gates_http_and_https_but_not_local_content() {
        assert!(!network_access_allows_url("http://example.test", false));
        assert!(!network_access_allows_url("https://example.test", false));
        assert!(network_access_allows_url("HTTP://example.test", true));
        assert!(network_access_allows_url("file:///game/page.html", false));
        assert!(network_access_allows_url("data:text/html,ok", false));
    }

    #[test]
    fn deferred_webview_requires_own_or_superview_extent() {
        assert!(!has_webview_extent((0.0, 0.0), None));
        assert!(!has_webview_extent((0.0, 0.0), Some((0.0, 0.0))));
        assert!(has_webview_extent((0.0, 0.0), Some((320.0, 480.0))));
        assert!(has_webview_extent((320.0, 480.0), None));
    }
}
