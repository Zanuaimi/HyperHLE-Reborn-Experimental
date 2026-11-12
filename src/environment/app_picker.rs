//! App picker GUI.

mod access_prompt;
mod archive_store;

use crate::bundle::Bundle;
use crate::frameworks::core_graphics::cg_affine_transform::{
    CGAffineTransform, CGAffineTransformIdentity,
};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, kCGImageAlphaPremultipliedLast};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_font::{
    UILineBreakModeWordWrap, UITextAlignmentCenter, UITextAlignmentLeft, UITextAlignmentRight,
};
use crate::frameworks::uikit::ui_gesture_recognizer::{
    UISwipeGestureRecognizerDirectionLeft, UISwipeGestureRecognizerDirectionRight,
};
use crate::frameworks::uikit::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::uikit::ui_keyboard::{
    UIKeyboardTypeDefault, UIKeyboardTypeNumbersAndPunctuation, UIReturnKeyDone,
};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::UIButtonTypeCustom;
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchCancel, UIControlEventTouchDown, UIControlEventTouchDragExit,
    UIControlEventTouchDragInside, UIControlEventTouchDragOutside, UIControlEventTouchUpInside,
    UIControlEventTouchUpOutside, UIControlEventValueChanged, UIControlStateNormal,
};
use crate::fs::BundleData;
use crate::image::Image;
use crate::mem::Ptr;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::options::{
    Button, Options, PerformanceOverlayCorner, PerformanceOverlayMode, PresentMode, RendererMode,
    VsyncMode,
};
use crate::paths;
use crate::window::{DeviceFamily, DeviceOrientation};
use crate::Environment;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicI8, Ordering};
use std::time::{Duration, Instant};

/// Pending left (-1) or right (+1) arrow key press, 0 if none. Written by the
/// window event handler and consumed by the picker loop.
static PENDING_ARROW_KEY: AtomicI8 = AtomicI8::new(0);
static PENDING_MOUSE_WHEEL: AtomicI32 = AtomicI32::new(0);

/// Records an arrow key press for the app picker to act on.
pub fn post_arrow_key(dir: i8) {
    PENDING_ARROW_KEY.store(dir, Ordering::Relaxed);
}

pub fn post_mouse_wheel(delta: i32) {
    PENDING_MOUSE_WHEEL.fetch_add(delta, Ordering::Relaxed);
}

struct AppInfo {
    path: PathBuf,
    display_name: String,
    /// `CFBundleIdentifier`, which also names the app's sandbox directory.
    bundle_id: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}

const LAST_APP_FILE: &str = "touchHLE_last_app.txt";

fn last_app_record_path() -> PathBuf {
    paths::user_data_base_path().join(LAST_APP_FILE)
}

fn read_last_app_path() -> Option<PathBuf> {
    let value = std::fs::read_to_string(last_app_record_path()).ok()?;
    Some(PathBuf::from(value.trim()))
}

fn remember_last_app_path(path: &Path) {
    if let Err(error) = std::fs::write(last_app_record_path(), path.to_string_lossy().as_bytes()) {
        log!("Warning: couldn't save last launched app path: {error}");
    }
}

pub fn app_picker(
    options: Options,
    show_lock_screen: bool,
) -> Result<(PathBuf, Vec<String>), String> {
    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);

    let apps: Result<Vec<AppInfo>, String> = if !apps_dir.is_dir() {
        Err(format!("The {} directory couldn't be found. Check you're running touchHLE from the right directory.", apps_dir.display()))
    } else {
        enumerate_apps(&apps_dir).map_err(|err| {
            format!(
                "Couldn't get list of apps in the {} directory: {}.",
                apps_dir.display(),
                err
            )
        })
    };

    show_app_picker_gui(options, apps, show_lock_screen)
}

fn enumerate_apps(apps_dir: &Path) -> Result<Vec<AppInfo>, std::io::Error> {
    let mut apps = Vec::new();
    let mut directories = vec![apps_dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let app_path = entry?.path();
            let extension = app_path.extension();
            if extension == Some(OsStr::new("app")) || extension == Some(OsStr::new("ipa")) {
                let (bundle, fs) = match BundleData::open_any(&app_path).and_then(|bundle_data| {
                    Bundle::new_bundle_and_fs_from_host_path(
                        bundle_data,
                        /* read_only_mode: */ true,
                    )
                }) {
                    Ok(ok) => ok,
                    Err(e) => {
                        log!(
                            "Warning: couldn't open app bundle {}: {} (skipping)",
                            app_path.display(),
                            e
                        );
                        continue;
                    }
                };

                let display_name = bundle.display_name().to_owned();
                let bundle_id = bundle.bundle_identifier().to_owned();
                let icon = match bundle.load_icon(&fs) {
                    Ok(icon) => Some(icon),
                    Err(e) => {
                        log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                        None
                    }
                };

                apps.push(AppInfo {
                    path: app_path,
                    display_name,
                    bundle_id,
                    icon,
                    display_name_ns_string: None,
                    icon_ui_image: None,
                });
            } else if app_path.is_dir() {
                directories.push(app_path);
            }
        }
    }

    apps.sort_by_key(|app| app.display_name.to_uppercase());
    Ok(apps)
}

fn sqlite_file_belongs_to_bundle(
    file_name: &str,
    bundle_id: &str,
    other_bundle_ids: &[&str],
) -> bool {
    let target_prefix = format!("{}_", sqlite_namespace(bundle_id));
    if !file_name.starts_with(target_prefix.as_str()) {
        return false;
    }
    !other_bundle_ids.iter().any(|other_id| {
        if *other_id == bundle_id {
            return false;
        }
        let other_prefix = format!("{}_", sqlite_namespace(other_id));
        other_prefix.len() > target_prefix.len() && file_name.starts_with(other_prefix.as_str())
    })
}

/// Deletes an app completely, like iOS does when an app and its data are
/// removed: the `.app` folder or `.ipa` file in the apps directory, the
/// sandbox folder (`Documents`, `Library`, ...) and the SQLite databases.
fn delete_app(app: &AppInfo, all_apps: &[AppInfo]) -> Result<(), String> {
    // The app goes first. If that fails, its data is kept.
    let removed = if app.path.is_dir() {
        std::fs::remove_dir_all(&app.path)
    } else {
        std::fs::remove_file(&app.path)
    };
    removed.map_err(|e| format!("couldn't delete {}: {}", app.path.display(), e))?;
    echo!("Deleted app: {}", app.path.display());

    // If another installed bundle/IPA shares the exact same bundle identifier,
    // keep the shared sandbox and SQLite databases intact.
    if all_apps
        .iter()
        .any(|other| other.path != app.path && other.bundle_id == app.bundle_id)
    {
        log!(
            "Keeping shared app data for {:?} because another bundle with the same ID remains installed",
            app.bundle_id
        );
        return Ok(());
    }

    // The app is already gone, so failures from here on are only logged.
    if is_plain_name(&app.bundle_id) {
        let sandbox_dir = paths::user_data_base_path()
            .join(paths::SANDBOX_DIR)
            .join(&app.bundle_id);
        if sandbox_dir.exists() {
            match std::fs::remove_dir_all(&sandbox_dir) {
                Ok(()) => echo!("Deleted app data: {}", sandbox_dir.display()),
                Err(e) => log!("Warning: couldn't delete {}: {}", sandbox_dir.display(), e),
            }
        }
    } else {
        log!(
            "Warning: not deleting the data of {:?}, its bundle identifier is not a plain name",
            app.bundle_id
        );
    }

    // touchHLE names each database `{namespace}_{file name}`. Exclude longer
    // sibling namespaces (`com.foo.game_hd_` when deleting `com.foo.game`).
    let other_bundle_ids: Vec<&str> = all_apps
        .iter()
        .filter(|other| other.path != app.path)
        .map(|other| other.bundle_id.as_str())
        .collect();
    let sqlite_dir = paths::user_data_base_path().join(paths::SQLITE_DIR);
    if let Ok(entries) = std::fs::read_dir(&sqlite_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if sqlite_file_belongs_to_bundle(&file_name, &app.bundle_id, &other_bundle_ids)
                && path.is_file()
            {
                if let Err(e) = std::fs::remove_file(&path) {
                    log!("Warning: couldn't delete {}: {}", path.display(), e);
                }
            }
        }
    }
    Ok(())
}

/// The name that libsqlite3.rs uses for an app's databases.
fn sqlite_namespace(bundle_id: &str) -> String {
    bundle_id
        .replace('/', "_")
        .replace('\\', "_")
        .replace(':', "_")
        .replace(' ', "_")
}

/// Whether `name` is one ordinary path component, so that joining it onto a
/// directory can't point somewhere else (e.g. `..` or `a/b`).
fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && matches!(
            (components.next(), components.next()),
            (Some(std::path::Component::Normal(_)), None)
        )
}

#[cfg(test)]
mod delete_app_tests {
    use super::*;

    #[test]
    fn sqlite_namespace_matches_libsqlite3() {
        assert_eq!(sqlite_namespace("com.example.game"), "com.example.game");
        assert_eq!(
            sqlite_namespace("com.example.my game"),
            "com.example.my_game"
        );
    }

    #[test]
    fn only_plain_names_are_accepted() {
        assert!(is_plain_name("com.example.game"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("."));
        assert!(!is_plain_name(".."));
        assert!(!is_plain_name("a/b"));
        assert!(!is_plain_name("a\\b"));
    }

    #[test]
    fn sqlite_prefix_excludes_longer_sibling_bundle_id() {
        let others = ["com.foo.game_hd", "com.bar.other"];
        assert!(sqlite_file_belongs_to_bundle(
            "com.foo.game_save.sqlite",
            "com.foo.game",
            &others
        ));
        assert!(!sqlite_file_belongs_to_bundle(
            "com.foo.game_hd_save.sqlite",
            "com.foo.game",
            &others
        ));
    }
}

/// Watch state for detecting a newly copied-in .ipa file.
struct IpaWatch {
    last_seen: Vec<(String, u64)>,
    dirty: bool,
    last_change: Option<Instant>,
}

/// List the .ipa files directly inside the apps directory, as (name, size).
///
/// The size matters: a file that is still being copied grows, and must not
/// be opened until it is complete. This is cheap enough to poll every
/// run-loop iteration, unlike a full [enumerate_apps], which opens every
/// app bundle.
fn list_top_level_ipa_files(apps_dir: &Path) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(apps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("ipa")) {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((name, size));
            }
        }
    }
    files.sort();
    files
}

/// How long the .ipa listing must stay unchanged before the app grid is
/// refreshed, so that a file that is still being copied is left alone.
const IPA_COPY_SETTLE_TIME: Duration = Duration::from_millis(500);

#[derive(Default)]
struct AppPickerDelegateHostObject {
    icon_tapped: id,
    // Set by the add-app tile; kept separately from icon_tapped because the
    // picker needs to wait for a newly copied IPA to settle before reloading.
    add_ipa: bool,
    recent_app_tapped: bool,
    status_accessory_tapped: bool,
    access_prompt_confirm: bool,
    access_prompt_cancel: bool,
    access_prompt_submit: bool,
    // Actions used by the built-in Internet Archive store screen.
    archive_store_close: bool,
    archive_store_search: bool,
    archive_store_previous: bool,
    archive_store_next: bool,
    archive_store_back: bool,
    archive_store_retry: bool,
    archive_store_cancel: bool,
    archive_store_item: Option<NSInteger>,
    archive_store_download: Option<NSInteger>,
    archive_store_tab: Option<NSInteger>,
    archive_store_sort: bool,
    archive_store_fuzzy: bool,
    archive_store_queue_action: Option<NSInteger>,
    archive_store_tap: Option<CGPoint>,
    settings_show: bool,
    settings_hide: bool,
    scale_hack_default: bool,
    scale_hack1: bool,
    scale_hack2: bool,
    scale_hack3: bool,
    scale_hack4: bool,
    orientation_default: bool,
    orientation_landscape_left: bool,
    orientation_landscape_right: bool,
    orientation_portrait_upside_down: bool,
    analog_stick_tilt_controls: Option<bool>,
    network: Option<bool>,
    game_center_authenticated: Option<bool>,
    show_fps: Option<bool>,
    trace_gl_errors: Option<bool>,
    trace_gl_draws: Option<bool>,
    vsync: Option<VsyncMode>,
    present_mode: Option<PresentMode>,
    fix_texture_min_filter: Option<Option<bool>>,
    present_rotate: Option<Option<u32>>,
    print_fps: Option<bool>,
    popup_errors: Option<bool>,
    controller_to_touch: Option<Option<bool>>,
    manual_value: Option<(NSInteger, String)>,
    overlay_mode: Option<PerformanceOverlayMode>,
    overlay_corner: Option<PerformanceOverlayCorner>,
    cheat_engine: Option<bool>,
    /// `Some(..)` only when the user just flipped the switch: `None` means no
    /// change is pending. The saved state is tri-state (see `PickerSettings`).
    rotation_fix: Option<bool>,
    /// `Some(..)` only when the user just flipped the switch.
    fix_movie_player: Option<bool>,
    corruption_mode: Option<CorruptionMode>,
    corruption_mode_changed: bool,
    corruption_startup_delay: Option<StartupDelayPreset>,
    corruption_cooldown: Option<CooldownPreset>,
    preserve_aspect_ratio: Option<bool>,
    show_lock_screen: Option<bool>,
    force_composition: Option<bool>,
    renderer_mode_tag: Option<i32>,
    fullscreen: Option<bool>,
    device_model_tag: Option<i32>,
    device_model_toggle: bool,
    device_model_scroll_to: Option<CGFloat>,
    language_tag: Option<i32>,
    language_toggle: bool,
    language_scroll_to: Option<CGFloat>,
    renderer_mode_toggle: bool,
    renderer_mode_scroll_to: Option<CGFloat>,
    gles2_mode: Option<Gles2Mode>,
    /// Set when a finger goes down on an app icon (`UIControlEventTouchDown`).
    icon_touch_down: id,
    /// Set when a touch that started on an icon stops being a tap.
    icon_touch_cancelled: bool,
    delete_cancel: bool,
    delete_confirm: bool,
    /// Set when the user taps the reset row in Settings.
    settings_reset: bool,
    /// Set when the user confirms resetting the settings to their defaults.
    reset_confirm: bool,
    /// Set when the "settings couldn't be saved" notice is dismissed.
    save_error_dismiss: bool,
    /// Set when the horizontal swipe recognizers fire (old-iOS page change).
    swipe_left: bool,
    swipe_right: bool,
    lock_screen_unlocked: bool,
    lock_slider_progress: f32,
}
impl HostObject for AppPickerDelegateHostObject {}

#[derive(Clone, PartialEq)]
struct PickerSettings {
    scale_hack: Option<NonZeroU32>,
    orientation: Option<DeviceOrientation>,
    fullscreen: bool,
    preserve_aspect_ratio: bool,
    renderer_mode: RendererMode,
    show_lock_screen: bool,
    force_composition: bool,
    show_fps: bool,
    trace_gl_errors: bool,
    trace_gl_draws: bool,
    vsync: VsyncMode,
    present_mode: PresentMode,
    fix_texture_min_filter: Option<bool>,
    present_rotate: Option<u32>,
    print_fps: bool,
    popup_errors: bool,
    deadzone: f32,
    x_tilt_range: f32,
    y_tilt_range: f32,
    x_tilt_offset: f32,
    y_tilt_offset: f32,
    /// `None` lets per-app option profiles provide controller mappings.
    controller_to_touch: Option<bool>,
    /// Manual fields only override profiles after the user commits an edit.
    manual_value_overrides: BTreeSet<NSInteger>,
    button_to_touch: HashMap<Button, (f32, f32)>,
    dpad_to_touch: Option<(f32, f32, f32, f32)>,
    stick_to_touch: Option<(f32, f32, f32, f32)>,
    fps_limit: Option<f64>,
    overlay_mode: PerformanceOverlayMode,
    overlay_corner: PerformanceOverlayCorner,
    /// `None` leaves these compatibility fixes to the command line and the
    /// app-specific options files. `Some(..)` is an explicit override from the
    /// Settings screen, including an explicit "off" that must be able to beat
    /// an options file which enables the fix (e.g. GTA III defaults).
    rotation_fix: Option<bool>,
    fix_movie_player: Option<bool>,
    corruption_mode: CorruptionMode,
    corruption_startup_delay: StartupDelayPreset,
    corruption_cooldown: CooldownPreset,
    corruption_bytes_per_burst: u32,
    corruption_max_offset: Option<u32>,
    corruption_seed: u64,
    gles2_mode: Gles2Mode,
    cheat_engine: bool,
    network: bool,
    game_center_authenticated: bool,
    analog_stick_tilt_controls: bool,
    device_model_tag: Option<i32>,
    language_tag: Option<i32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Gles2Mode {
    #[default]
    Off,
    PreferContext,
    Compatibility,
}

impl Gles2Mode {
    fn selected_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::PreferContext => 1,
            Self::Compatibility => 2,
        }
    }

    fn short_name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::PreferContext => "prefer-context",
            Self::Compatibility => "compatibility",
        }
    }

    fn from_short_name(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "prefer-context" => Some(Self::PreferContext),
            "compatibility" => Some(Self::Compatibility),
            _ => None,
        }
    }

    fn arguments(self) -> Vec<String> {
        match self {
            Self::Off => Vec::new(),
            Self::PreferContext => vec!["--prefer-gles2-context".to_string()],
            Self::Compatibility => vec!["--gles2-compat".to_string()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorruptionMode {
    Off,
    Corrupt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartupDelayPreset {
    #[default]
    Immediate,
    FiveSeconds,
    FifteenSeconds,
    ThirtySeconds,
}

impl StartupDelayPreset {
    const ALL: [Self; 4] = [
        Self::Immediate,
        Self::FiveSeconds,
        Self::FifteenSeconds,
        Self::ThirtySeconds,
    ];

    fn short_name(self) -> &'static str {
        match self {
            Self::Immediate => "0",
            Self::FiveSeconds => "5",
            Self::FifteenSeconds => "15",
            Self::ThirtySeconds => "30",
        }
    }

    fn from_short_name(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.short_name() == value)
    }

    fn selected_index(self) -> usize {
        Self::ALL.iter().position(|preset| *preset == self).unwrap()
    }

    fn duration(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Immediate => 0,
            Self::FiveSeconds => 5,
            Self::FifteenSeconds => 15,
            Self::ThirtySeconds => 30,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CooldownPreset {
    #[default]
    HalfSecond,
    OneSecond,
    FiveSeconds,
    TenSeconds,
}

impl CooldownPreset {
    const ALL: [Self; 4] = [
        Self::HalfSecond,
        Self::OneSecond,
        Self::FiveSeconds,
        Self::TenSeconds,
    ];

    fn short_name(self) -> &'static str {
        match self {
            Self::HalfSecond => "0.5",
            Self::OneSecond => "1",
            Self::FiveSeconds => "5",
            Self::TenSeconds => "10",
        }
    }

    fn from_short_name(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.short_name() == value)
    }

    fn selected_index(self) -> usize {
        Self::ALL.iter().position(|preset| *preset == self).unwrap()
    }

    fn duration(self) -> std::time::Duration {
        match self {
            Self::HalfSecond => std::time::Duration::from_millis(500),
            Self::OneSecond => std::time::Duration::from_secs(1),
            Self::FiveSeconds => std::time::Duration::from_secs(5),
            Self::TenSeconds => std::time::Duration::from_secs(10),
        }
    }
}

impl CorruptionMode {
    fn short_name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Corrupt => "corrupt",
        }
    }

    fn from_short_name(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "corrupt" => Some(Self::Corrupt),
            _ => None,
        }
    }

    fn option(self) -> Option<bool> {
        match self {
            Self::Off => None,
            Self::Corrupt => Some(true),
        }
    }

    fn selected_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Corrupt => 1,
        }
    }
}

fn saved_renderer_mode(contents: &str) -> Option<RendererMode> {
    let mut renderer_mode = None;
    let mut legacy_mode = None;
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "renderer_mode" => {
                if let Ok(mode) = RendererMode::from_short_name(value) {
                    renderer_mode = Some(mode);
                }
            }
            "gles_native" => match value {
                "true" => legacy_mode = Some(RendererMode::NativeGles),
                "false" => legacy_mode = Some(RendererMode::Angle),
                _ => (),
            },
            _ => (),
        }
    }
    renderer_mode.or(legacy_mode)
}

impl PickerSettings {
    fn load(options: &Options, angle_backend_available: bool) -> Self {
        let mut settings = Self::from_options(options, angle_backend_available);
        let path = paths::user_data_base_path().join(paths::PICKER_SETTINGS_FILE);
        let Ok(contents) = std::fs::read_to_string(path) else {
            return settings;
        };
        if let Some(mode) = saved_renderer_mode(&contents) {
            settings.renderer_mode = mode;
        }
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "scale_hack" => {
                    settings.scale_hack = match value {
                        "default" => None,
                        _ => value.parse().ok().and_then(NonZeroU32::new),
                    }
                }
                "orientation" => {
                    settings.orientation = match value {
                        "default" => None,
                        "landscape-left" => Some(DeviceOrientation::LandscapeLeft),
                        "landscape-right" => Some(DeviceOrientation::LandscapeRight),
                        "upside-down" => Some(DeviceOrientation::PortraitUpsideDown),
                        _ => settings.orientation,
                    }
                }
                "fullscreen" => set_bool(value, &mut settings.fullscreen),
                "preserve_aspect_ratio" => set_bool(value, &mut settings.preserve_aspect_ratio),
                "show_lock_screen" => set_bool(value, &mut settings.show_lock_screen),
                "force_composition" => set_bool(value, &mut settings.force_composition),
                "show_fps" => set_bool(value, &mut settings.show_fps),
                "trace_gl_errors" => set_bool(value, &mut settings.trace_gl_errors),
                "trace_gl_draws" => set_bool(value, &mut settings.trace_gl_draws),
                "vsync" => {
                    if let Ok(mode) = VsyncMode::from_short_name(value) {
                        settings.vsync = mode;
                    }
                }
                "present_mode" => {
                    if let Ok(mode) = PresentMode::from_short_name(value) {
                        settings.present_mode = mode;
                    }
                }
                "fix_texture_min_filter" => {
                    set_bool_override(value, &mut settings.fix_texture_min_filter)
                }
                "present_rotate" => {
                    settings.present_rotate = match value {
                        "default" => None,
                        _ => value
                            .parse::<u32>()
                            .ok()
                            .filter(|degree| matches!(degree, 0 | 90 | 180 | 270))
                            .or(settings.present_rotate),
                    }
                }
                "print_fps" => set_bool(value, &mut settings.print_fps),
                "popup_errors" => set_bool(value, &mut settings.popup_errors),
                "deadzone" => {
                    settings.deadzone =
                        parse_clamped_f32(value, 0.0, 0.95).unwrap_or(settings.deadzone)
                }
                "x_tilt_range" => {
                    settings.x_tilt_range =
                        parse_clamped_f32(value, -360.0, 360.0).unwrap_or(settings.x_tilt_range)
                }
                "y_tilt_range" => {
                    settings.y_tilt_range =
                        parse_clamped_f32(value, -360.0, 360.0).unwrap_or(settings.y_tilt_range)
                }
                "x_tilt_offset" => {
                    settings.x_tilt_offset =
                        parse_clamped_f32(value, -360.0, 360.0).unwrap_or(settings.x_tilt_offset)
                }
                "y_tilt_offset" => {
                    settings.y_tilt_offset =
                        parse_clamped_f32(value, -360.0, 360.0).unwrap_or(settings.y_tilt_offset)
                }
                "controller_to_touch" => {
                    set_bool_override(value, &mut settings.controller_to_touch)
                }
                "manual_value_overrides" => {
                    settings.manual_value_overrides = parse_manual_value_overrides(value);
                }
                "button_to_touch" => {
                    if let Some(mappings) = parse_button_mappings(value) {
                        settings.button_to_touch = mappings;
                    }
                }
                "dpad_to_touch" => {
                    if let Some(mapping) = parse_touch_region(value) {
                        settings.dpad_to_touch = mapping;
                    }
                }
                "stick_to_touch" => {
                    if let Some(mapping) = parse_touch_region(value) {
                        settings.stick_to_touch = mapping;
                    }
                }
                "fps_limit" => {
                    if let Some(limit) = parse_fps_limit(value) {
                        settings.fps_limit = limit;
                    }
                }
                "corruption_bytes_per_burst" => {
                    settings.corruption_bytes_per_burst = parse_clamped_u32(value, 1, 64)
                        .unwrap_or(settings.corruption_bytes_per_burst)
                }
                "corruption_max_offset" => {
                    if let Some(max_offset) = parse_max_offset(value) {
                        settings.corruption_max_offset = max_offset;
                    }
                }
                "corruption_seed" => {
                    settings.corruption_seed =
                        parse_clamped_u64(value, 0, u64::MAX).unwrap_or(settings.corruption_seed)
                }
                "overlay_mode" => {
                    if let Some(mode) = PerformanceOverlayMode::from_short_name(value) {
                        settings.overlay_mode = mode;
                    }
                }
                "overlay_corner" => {
                    if let Some(corner) = PerformanceOverlayCorner::from_short_name(value) {
                        settings.overlay_corner = corner;
                    }
                }
                "rotation_fix" => set_bool_override(value, &mut settings.rotation_fix),
                "fix_movie_player" => set_bool_override(value, &mut settings.fix_movie_player),
                "corruption_mode" => {
                    if let Some(mode) = CorruptionMode::from_short_name(value) {
                        settings.corruption_mode = mode;
                    }
                }
                "corruption_startup_delay" => {
                    if let Some(preset) = StartupDelayPreset::from_short_name(value) {
                        settings.corruption_startup_delay = preset;
                    }
                }
                "corruption_cooldown" => {
                    if let Some(preset) = CooldownPreset::from_short_name(value) {
                        settings.corruption_cooldown = preset;
                    }
                }
                "gles2_mode" => {
                    if let Some(mode) = Gles2Mode::from_short_name(value) {
                        settings.gles2_mode = mode;
                    }
                }
                "cheat_engine" => set_bool(value, &mut settings.cheat_engine),
                "network" => set_bool(value, &mut settings.network),
                "game_center_authenticated" => {
                    set_bool(value, &mut settings.game_center_authenticated)
                }
                "analog_stick_tilt_controls" => {
                    set_bool(value, &mut settings.analog_stick_tilt_controls)
                }
                "device_model_tag" => settings.device_model_tag = parse_optional_tag(value),
                "language_tag" => settings.language_tag = parse_optional_tag(value),
                _ => (),
            }
        }
        settings.renderer_mode =
            quick_options_renderer_mode_state(settings.renderer_mode, angle_backend_available).0;
        settings.overlay_mode = settings.overlay_mode.for_current_platform();
        let (width, height) = picker_touch_screen_size(options, &settings);
        clamp_touch_mappings(&mut settings, width, height);
        settings
    }

    /// The settings a fresh install starts with, and the settings that
    /// "Reset all settings" restores. Both must match, or a reset would leave
    /// the picker looking different from a first launch.
    ///
    /// Only the emulator's own factory values are used; command-line flags and
    /// app-specific options files are deliberately ignored here, because the
    /// picker's saved values are layered on top of them at launch.
    fn factory_defaults(angle_backend_available: bool) -> Self {
        Self::from_options(&Options::default(), angle_backend_available)
    }

    fn from_options(options: &Options, angle_backend_available: bool) -> Self {
        let (renderer_mode, _) =
            quick_options_renderer_mode_state(options.renderer_mode, angle_backend_available);
        Self {
            scale_hack: None,
            orientation: None,
            fullscreen: options.fullscreen,
            preserve_aspect_ratio: options.preserve_aspect_ratio,
            renderer_mode,
            show_lock_screen: true,
            force_composition: options.force_composition,
            show_fps: options.performance_overlay_enabled,
            trace_gl_errors: options.trace_gl_errors,
            trace_gl_draws: options.trace_gl_draws,
            vsync: options.vsync,
            present_mode: options.present_mode,
            fix_texture_min_filter: None,
            present_rotate: options.present_rotate,
            print_fps: options.print_fps,
            popup_errors: options.popup_errors,
            deadzone: options.deadzone.clamp(0.0, 0.95),
            x_tilt_range: options.x_tilt_range.clamp(-360.0, 360.0),
            y_tilt_range: options.y_tilt_range.clamp(-360.0, 360.0),
            x_tilt_offset: options.x_tilt_offset.clamp(-360.0, 360.0),
            y_tilt_offset: options.y_tilt_offset.clamp(-360.0, 360.0),
            controller_to_touch: None,
            manual_value_overrides: BTreeSet::new(),
            button_to_touch: options.button_to_touch.clone(),
            dpad_to_touch: options.dpad_to_touch,
            stick_to_touch: options.stick_to_touch,
            fps_limit: options.fps_limit,
            overlay_mode: options.performance_overlay_mode.for_current_platform(),
            overlay_corner: options.performance_overlay_corner,
            // An explicit CLI choice is preserved; otherwise app profiles can
            // continue to supply their compatibility defaults at launch.
            rotation_fix: options.rotation_fix.then_some(true),
            fix_movie_player: options.fix_movie_player.then_some(true),
            corruption_mode: CorruptionMode::Off,
            corruption_startup_delay: StartupDelayPreset::default(),
            corruption_cooldown: CooldownPreset::default(),
            corruption_bytes_per_burst: options.corruption.bytes_per_burst.clamp(1, 64),
            corruption_max_offset: options
                .corruption
                .max_offset
                .map(|value| value.clamp(1, 1_048_576)),
            corruption_seed: options.corruption.seed,
            gles2_mode: Gles2Mode::Off,
            cheat_engine: quick_options_trainer_enabled(options),
            network: options.network_access,
            game_center_authenticated: options.game_center_authenticated,
            analog_stick_tilt_controls: options.analog_stick_tilt_controls,
            device_model_tag: None,
            language_tag: None,
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let path = paths::user_data_base_path().join(paths::PICKER_SETTINGS_FILE);
        let orientation = match self.orientation {
            None => "default",
            Some(DeviceOrientation::LandscapeLeft) => "landscape-left",
            Some(DeviceOrientation::LandscapeRight) => "landscape-right",
            Some(DeviceOrientation::PortraitUpsideDown) => "upside-down",
            Some(DeviceOrientation::Portrait) => "default",
        };
        let contents = format!(
            "# Saved by the graphical settings screen.\n\
             scale_hack={}\n\
             orientation={orientation}\n\
             fullscreen={}\n\
             preserve_aspect_ratio={}\n\
             renderer_mode={}\n\
             show_lock_screen={}\n\
             force_composition={}\n\
             show_fps={}\n\
             trace_gl_errors={}\n\
             trace_gl_draws={}\n\
             vsync={}\n\
             present_mode={}\n\
             fix_texture_min_filter={}\n\
             present_rotate={}\n\
             print_fps={}\n\
             popup_errors={}\n\
             deadzone={}\n\
             x_tilt_range={}\n\
             y_tilt_range={}\n\
             x_tilt_offset={}\n\
             y_tilt_offset={}\n\
             controller_to_touch={}\n\
             manual_value_overrides={}\n\
             button_to_touch={}\n\
             dpad_to_touch={}\n\
             stick_to_touch={}\n\
             fps_limit={}\n\
             overlay_mode={}\n\
             overlay_corner={}\n\
             rotation_fix={}\n\
             fix_movie_player={}\n\
             corruption_mode={}\n\
             corruption_startup_delay={}\n\
             corruption_cooldown={}\n\
             corruption_bytes_per_burst={}\n\
             corruption_max_offset={}\n\
             corruption_seed={}\n\
             gles2_mode={}\n\
             cheat_engine={}\n\
             network={}\n\
             game_center_authenticated={}\n\
             analog_stick_tilt_controls={}\n\
             device_model_tag={}\n\
             language_tag={}\n",
            self.scale_hack
                .map_or_else(|| "default".to_string(), |n| n.to_string()),
            self.fullscreen,
            self.preserve_aspect_ratio,
            self.renderer_mode.short_name(),
            self.show_lock_screen,
            self.force_composition,
            self.show_fps,
            self.trace_gl_errors,
            self.trace_gl_draws,
            vsync_short_name(self.vsync),
            present_mode_short_name(self.present_mode),
            optional_bool(self.fix_texture_min_filter),
            self.present_rotate
                .map_or_else(|| "default".to_string(), |value| value.to_string()),
            self.print_fps,
            self.popup_errors,
            self.deadzone,
            self.x_tilt_range,
            self.y_tilt_range,
            self.x_tilt_offset,
            self.y_tilt_offset,
            optional_bool(self.controller_to_touch),
            self.manual_value_overrides
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            format_button_mappings(&self.button_to_touch),
            format_touch_region(self.dpad_to_touch),
            format_touch_region(self.stick_to_touch),
            self.fps_limit
                .map_or_else(|| "off".to_string(), |value| value.to_string()),
            self.overlay_mode.short_name(),
            self.overlay_corner.short_name(),
            optional_bool(self.rotation_fix),
            optional_bool(self.fix_movie_player),
            self.corruption_mode.short_name(),
            self.corruption_startup_delay.short_name(),
            self.corruption_cooldown.short_name(),
            self.corruption_bytes_per_burst,
            self.corruption_max_offset
                .map_or_else(|| "unbounded".to_string(), |value| value.to_string()),
            self.corruption_seed,
            self.gles2_mode.short_name(),
            self.cheat_engine,
            self.network,
            self.game_center_authenticated,
            self.analog_stick_tilt_controls,
            optional_tag(self.device_model_tag),
            optional_tag(self.language_tag),
        );
        atomic_write_picker_settings(&path, contents.as_bytes())
    }
}

fn atomic_write_picker_settings(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    loop {
        let number = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".picker-settings-{}-{number}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = file.write_all(contents).and_then(|()| file.sync_all());
        drop(file);
        let result = result.and_then(|()| std::fs::rename(&temp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        return result;
    }
}

fn set_bool(value: &str, setting: &mut bool) {
    match value {
        "true" => *setting = true,
        "false" => *setting = false,
        _ => (),
    }
}

/// Reads a tri-state setting from the picker settings file. Only explicit
/// `true`/`false` values are overrides; `default` (or an unknown value) leaves
/// the value derived from the command line untouched, so a CLI flag is not
/// silently cancelled by a saved "untouched switch" line.
fn set_bool_override(value: &str, setting: &mut Option<bool>) {
    match value {
        "true" => *setting = Some(true),
        "false" => *setting = Some(false),
        _ => (),
    }
}

fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        None => "default",
        Some(true) => "true",
        Some(false) => "false",
    }
}

fn parse_optional_tag(value: &str) -> Option<i32> {
    if value == "default" {
        None
    } else {
        value.parse().ok()
    }
}

fn vsync_short_name(mode: VsyncMode) -> &'static str {
    match mode {
        VsyncMode::Auto => "auto",
        VsyncMode::On => "on",
        VsyncMode::Off => "off",
    }
}

fn present_mode_short_name(mode: PresentMode) -> &'static str {
    match mode {
        PresentMode::Auto => "auto",
        PresentMode::Direct => "direct",
        PresentMode::Readback => "readback",
    }
}

fn vsync_selected_index(mode: VsyncMode) -> usize {
    match mode {
        VsyncMode::Auto => 0,
        VsyncMode::On => 1,
        VsyncMode::Off => 2,
    }
}

fn present_mode_selected_index(mode: PresentMode) -> usize {
    match mode {
        PresentMode::Auto => 0,
        PresentMode::Direct => 1,
        PresentMode::Readback => 2,
    }
}

fn texture_filter_selected_index(value: Option<bool>) -> usize {
    match value {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

fn present_rotate_selected_index(value: Option<u32>) -> usize {
    match value {
        None => 0,
        Some(0) => 1,
        Some(90) => 2,
        Some(180) => 3,
        Some(270) => 4,
        Some(_) => 0,
    }
}

fn controller_to_touch_selected_index(value: Option<bool>) -> usize {
    match value {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

fn parse_button(value: &str) -> Option<Button> {
    match value.trim() {
        "DPadLeft" => Some(Button::DPadLeft),
        "DPadUp" => Some(Button::DPadUp),
        "DPadRight" => Some(Button::DPadRight),
        "DPadDown" => Some(Button::DPadDown),
        "Start" => Some(Button::Start),
        "A" => Some(Button::A),
        "B" => Some(Button::B),
        "X" => Some(Button::X),
        "Y" => Some(Button::Y),
        "LeftShoulder" => Some(Button::LeftShoulder),
        _ => None,
    }
}

fn button_name(button: Button) -> &'static str {
    match button {
        Button::DPadLeft => "DPadLeft",
        Button::DPadUp => "DPadUp",
        Button::DPadRight => "DPadRight",
        Button::DPadDown => "DPadDown",
        Button::Start => "Start",
        Button::A => "A",
        Button::B => "B",
        Button::X => "X",
        Button::Y => "Y",
        Button::LeftShoulder => "LeftShoulder",
    }
}

const CONTROLLER_BUTTONS: [Button; 10] = [
    Button::DPadLeft,
    Button::DPadUp,
    Button::DPadRight,
    Button::DPadDown,
    Button::Start,
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::LeftShoulder,
];

fn format_button_mappings(mappings: &HashMap<Button, (f32, f32)>) -> String {
    CONTROLLER_BUTTONS
        .iter()
        .filter_map(|button| {
            mappings
                .get(button)
                .map(|(x, y)| format!("{},{x},{y}", button_name(*button)))
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn parse_button_mappings(value: &str) -> Option<HashMap<Button, (f32, f32)>> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("none") {
        return Some(HashMap::new());
    }
    let mut mappings = HashMap::new();
    for entry in value.split(';') {
        let mut fields = entry.split(',');
        let button = parse_button(fields.next()?)?;
        let x = parse_finite_f32(fields.next()?)?;
        let y = parse_finite_f32(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        mappings.insert(button, (x, y));
    }
    Some(mappings)
}

fn parse_touch_region(value: &str) -> Option<Option<(f32, f32, f32, f32)>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    let values: Vec<f32> = value
        .split(',')
        .map(parse_finite_f32)
        .collect::<Option<_>>()?;
    let [x, y, width, height]: [f32; 4] = values.try_into().ok()?;
    Some(Some((x, y, width, height)))
}

fn format_touch_region(region: Option<(f32, f32, f32, f32)>) -> String {
    region.map_or_else(
        || "off".to_string(),
        |(x, y, width, height)| format!("{x},{y},{width},{height}"),
    )
}

fn parse_finite_f32(value: &str) -> Option<f32> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| !value.is_nan())
        .map(|value| value.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32)
}

fn parse_clamped_f32(value: &str, minimum: f32, maximum: f32) -> Option<f32> {
    Some(parse_finite_f32(value)?.clamp(minimum, maximum))
}

fn parse_clamped_u32(value: &str, minimum: u32, maximum: u32) -> Option<u32> {
    Some(parse_clamped_unsigned(value, u64::from(minimum), u64::from(maximum))? as u32)
}

fn parse_clamped_u64(value: &str, minimum: u64, maximum: u64) -> Option<u64> {
    parse_clamped_unsigned(value, minimum, maximum)
}

fn parse_clamped_unsigned(value: &str, minimum: u64, maximum: u64) -> Option<u64> {
    let value = value.trim();
    if let Some(digits) = value.strip_prefix('-') {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        return Some(minimum);
    }
    let digits = value.strip_prefix('+').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = digits.parse::<u128>().unwrap_or(u128::MAX);
    Some(value.clamp(u128::from(minimum), u128::from(maximum)) as u64)
}

fn parse_fps_limit(value: &str) -> Option<Option<f64>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("unlimited") {
        return Some(None);
    }
    let value = value.parse::<f64>().ok().filter(|value| !value.is_nan())?;
    Some(Some(value.clamp(1.0, 240.0)))
}

fn parse_max_offset(value: &str) -> Option<Option<u32>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("unbounded") {
        return Some(None);
    }
    Some(Some(parse_clamped_u32(value, 1, 1_048_576)?))
}

fn format_optional_float(value: Option<f64>) -> String {
    value.map_or_else(|| "off".to_string(), |value| value.to_string())
}

fn picker_touch_screen_size(options: &Options, settings: &PickerSettings) -> (f32, f32) {
    let family = settings
        .device_model_tag
        .filter(|tag| *tag != DEVICE_TAG_DEFAULT && *tag != DEVICE_TAG_AUTO)
        .and_then(|tag| usize::try_from(tag).ok())
        .and_then(DeviceFamily::for_tag)
        .or(options.device_family)
        .unwrap_or(DeviceFamily::iPhone);
    let (mut width, mut height) = options
        .host_screen_size
        .unwrap_or_else(|| family.portrait_size());
    let orientation = settings.orientation.unwrap_or(options.initial_orientation);
    if matches!(
        orientation,
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
    ) {
        std::mem::swap(&mut width, &mut height);
    }
    (width as f32, height as f32)
}

fn clamp_touch_mappings(settings: &mut PickerSettings, width: f32, height: f32) {
    for (x, y) in settings.button_to_touch.values_mut() {
        *x = clamp_screen_coordinate(*x, width);
        *y = clamp_screen_coordinate(*y, height);
    }
    for mapping in [&mut settings.dpad_to_touch, &mut settings.stick_to_touch] {
        if let Some((x, y, region_width, region_height)) = mapping {
            *x = clamp_screen_coordinate(*x, width);
            *y = clamp_screen_coordinate(*y, height);
            *region_width = clamp_screen_coordinate(*region_width, width - *x);
            *region_height = clamp_screen_coordinate(*region_height, height - *y);
        }
    }
}

fn clamp_screen_coordinate(value: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, maximum)
    } else {
        0.0
    }
}

fn manual_button_tag(button: Button) -> NSInteger {
    MANUAL_BUTTON_TOUCH_BASE
        + CONTROLLER_BUTTONS
            .iter()
            .position(|candidate| *candidate == button)
            .unwrap() as NSInteger
}

fn manual_button_for_tag(tag: NSInteger) -> Option<Button> {
    let index = usize::try_from(tag.checked_sub(MANUAL_BUTTON_TOUCH_BASE)?).ok()?;
    CONTROLLER_BUTTONS.get(index).copied()
}

fn is_manual_value_tag(tag: NSInteger) -> bool {
    matches!(
        tag,
        MANUAL_DEADZONE
            | MANUAL_X_TILT_RANGE
            | MANUAL_Y_TILT_RANGE
            | MANUAL_X_TILT_OFFSET
            | MANUAL_Y_TILT_OFFSET
            | MANUAL_DPAD_TOUCH
            | MANUAL_STICK_TOUCH
            | MANUAL_FPS_LIMIT
            | MANUAL_CORRUPTION_INTENSITY
            | MANUAL_CORRUPTION_MAX_OFFSET
            | MANUAL_CORRUPTION_SEED
    ) || manual_button_for_tag(tag).is_some()
}

fn parse_manual_value_overrides(value: &str) -> BTreeSet<NSInteger> {
    value
        .split(',')
        .filter_map(|tag| tag.parse::<NSInteger>().ok())
        .filter(|tag| is_manual_value_tag(*tag))
        .collect()
}

fn is_controller_mapping_tag(tag: NSInteger) -> bool {
    matches!(tag, MANUAL_DPAD_TOUCH | MANUAL_STICK_TOUCH) || manual_button_for_tag(tag).is_some()
}

fn manual_value_text(settings: &PickerSettings, tag: NSInteger) -> Option<String> {
    if is_controller_mapping_tag(tag) && settings.controller_to_touch.is_none() {
        return Some("App default".to_string());
    }
    if matches!(
        tag,
        MANUAL_X_TILT_RANGE | MANUAL_Y_TILT_RANGE | MANUAL_X_TILT_OFFSET | MANUAL_Y_TILT_OFFSET
    ) && !settings.manual_value_overrides.contains(&tag)
    {
        return Some("App default".to_string());
    }
    Some(match tag {
        MANUAL_DEADZONE => settings.deadzone.to_string(),
        MANUAL_X_TILT_RANGE => settings.x_tilt_range.to_string(),
        MANUAL_Y_TILT_RANGE => settings.y_tilt_range.to_string(),
        MANUAL_X_TILT_OFFSET => settings.x_tilt_offset.to_string(),
        MANUAL_Y_TILT_OFFSET => settings.y_tilt_offset.to_string(),
        MANUAL_DPAD_TOUCH => format_touch_region(settings.dpad_to_touch),
        MANUAL_STICK_TOUCH => format_touch_region(settings.stick_to_touch),
        MANUAL_FPS_LIMIT => settings
            .fps_limit
            .map_or_else(|| "Unlimited".to_string(), |value| value.to_string()),
        MANUAL_CORRUPTION_INTENSITY => settings.corruption_bytes_per_burst.to_string(),
        MANUAL_CORRUPTION_MAX_OFFSET => settings
            .corruption_max_offset
            .map_or_else(|| "Unbounded".to_string(), |value| value.to_string()),
        MANUAL_CORRUPTION_SEED => settings.corruption_seed.to_string(),
        _ => {
            let button = manual_button_for_tag(tag)?;
            settings
                .button_to_touch
                .get(&button)
                .map_or_else(|| "off".to_string(), |(x, y)| format!("{x},{y}"))
        }
    })
}

fn manual_value_description(tag: NSInteger, width: f32, height: f32) -> String {
    match tag {
        MANUAL_DEADZONE => "Default: 0.10; range: 0–0.95.".to_string(),
        MANUAL_X_TILT_RANGE | MANUAL_Y_TILT_RANGE => {
            "Default: 60°; range: −360° to 360°. Type App default to use profile values.".to_string()
        }
        MANUAL_X_TILT_OFFSET | MANUAL_Y_TILT_OFFSET => {
            "Default: 0°; range: −360° to 360°. Type App default to use profile values.".to_string()
        }
        MANUAL_DPAD_TOUCH | MANUAL_STICK_TOUCH => format!(
            "Default: App default; type off or x,y,w,h; region must stay inside 0–{width:.0} × 0–{height:.0} points."
        ),
        MANUAL_FPS_LIMIT => "Default: 60; enter 1–240 or Unlimited.".to_string(),
        MANUAL_CORRUPTION_INTENSITY => "Default: 8 bytes per burst; range: 1–64.".to_string(),
        MANUAL_CORRUPTION_MAX_OFFSET => {
            "Default: Unbounded; enter 1–1,048,576 or Unbounded.".to_string()
        }
        MANUAL_CORRUPTION_SEED => format!("Default: 0; range: 0–{}.", u64::MAX),
        _ => format!(
            "Default: App default; type off or x,y in points (x 0–{width:.0}, y 0–{height:.0})."
        ),
    }
}

fn apply_manual_value(
    settings: &mut PickerSettings,
    tag: NSInteger,
    text: &str,
    screen_width: f32,
    screen_height: f32,
) -> Option<String> {
    if text.trim().eq_ignore_ascii_case("app default") {
        if !is_controller_mapping_tag(tag)
            && !matches!(
                tag,
                MANUAL_X_TILT_RANGE
                    | MANUAL_Y_TILT_RANGE
                    | MANUAL_X_TILT_OFFSET
                    | MANUAL_Y_TILT_OFFSET
            )
        {
            return None;
        }
        settings.manual_value_overrides.remove(&tag);
        if is_controller_mapping_tag(tag) {
            for button in CONTROLLER_BUTTONS {
                settings
                    .manual_value_overrides
                    .remove(&manual_button_tag(button));
            }
            settings.manual_value_overrides.remove(&MANUAL_DPAD_TOUCH);
            settings.manual_value_overrides.remove(&MANUAL_STICK_TOUCH);
            settings.controller_to_touch = None;
        }
        return manual_value_text(settings, tag);
    }
    match tag {
        MANUAL_DEADZONE => settings.deadzone = parse_clamped_f32(text, 0.0, 0.95)?,
        MANUAL_X_TILT_RANGE => settings.x_tilt_range = parse_clamped_f32(text, -360.0, 360.0)?,
        MANUAL_Y_TILT_RANGE => settings.y_tilt_range = parse_clamped_f32(text, -360.0, 360.0)?,
        MANUAL_X_TILT_OFFSET => settings.x_tilt_offset = parse_clamped_f32(text, -360.0, 360.0)?,
        MANUAL_Y_TILT_OFFSET => settings.y_tilt_offset = parse_clamped_f32(text, -360.0, 360.0)?,
        MANUAL_DPAD_TOUCH | MANUAL_STICK_TOUCH => {
            let mapping = parse_touch_region(text)?;
            let target = if tag == MANUAL_DPAD_TOUCH {
                &mut settings.dpad_to_touch
            } else {
                &mut settings.stick_to_touch
            };
            *target = mapping;
            clamp_touch_mappings(settings, screen_width, screen_height);
        }
        MANUAL_FPS_LIMIT => settings.fps_limit = parse_fps_limit(text)?,
        MANUAL_CORRUPTION_INTENSITY => {
            settings.corruption_bytes_per_burst = parse_clamped_u32(text, 1, 64)?
        }
        MANUAL_CORRUPTION_MAX_OFFSET => settings.corruption_max_offset = parse_max_offset(text)?,
        MANUAL_CORRUPTION_SEED => settings.corruption_seed = parse_clamped_u64(text, 0, u64::MAX)?,
        _ => {
            let button = manual_button_for_tag(tag)?;
            let value = text.trim();
            if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("none") {
                settings.button_to_touch.remove(&button);
            } else {
                let mut values = value.split(',');
                let x = parse_finite_f32(values.next()?)?.clamp(0.0, screen_width);
                let y = parse_finite_f32(values.next()?)?.clamp(0.0, screen_height);
                if values.next().is_some() {
                    return None;
                }
                settings.button_to_touch.insert(button, (x, y));
            }
        }
    }
    settings.manual_value_overrides.insert(tag);
    if is_controller_mapping_tag(tag) {
        settings.controller_to_touch = Some(true);
    }
    manual_value_text(settings, tag)
}

fn manual_value_row(
    settings: &PickerSettings,
    tag: NSInteger,
    title: String,
    screen_width: f32,
    screen_height: f32,
    enabled: bool,
    text_keyboard: bool,
) -> SettingsRow {
    SettingsRow::ManualValue {
        title,
        description: manual_value_description(tag, screen_width, screen_height),
        tag,
        value: manual_value_text(settings, tag).unwrap(),
        enabled,
        text_keyboard,
    }
}

fn manual_value_enabled(tag: NSInteger, settings: &PickerSettings) -> bool {
    if tag == MANUAL_DEADZONE {
        settings
            .controller_to_touch
            .map_or(true, |enabled| enabled && settings.stick_to_touch.is_some())
    } else if matches!(
        tag,
        MANUAL_X_TILT_RANGE | MANUAL_Y_TILT_RANGE | MANUAL_X_TILT_OFFSET | MANUAL_Y_TILT_OFFSET
    ) {
        settings.analog_stick_tilt_controls
    } else if matches!(tag, MANUAL_DPAD_TOUCH | MANUAL_STICK_TOUCH)
        || manual_button_for_tag(tag).is_some()
    {
        settings.controller_to_touch != Some(false)
    } else if matches!(
        tag,
        MANUAL_CORRUPTION_INTENSITY | MANUAL_CORRUPTION_MAX_OFFSET | MANUAL_CORRUPTION_SEED
    ) {
        settings.corruption_mode == CorruptionMode::Corrupt
    } else {
        true
    }
}

fn refresh_manual_value_fields(
    env: &mut Environment,
    fields: &[(NSInteger, id, id)],
    width: f32,
    height: f32,
    settings: &mut PickerSettings,
) {
    clamp_touch_mappings(settings, width, height);
    for &(tag, field, description_label) in fields {
        if let Some(value) = manual_value_text(settings, tag) {
            let value = ns_string::from_rust_string(env, value);
            () = msg![env; field setText:value];
            release(env, value);
        }
        () = msg![env; field setEnabled:(manual_value_enabled(tag, settings))];
        let description =
            ns_string::from_rust_string(env, manual_value_description(tag, width, height));
        () = msg![env; description_label setText:description];
        release(env, description);
    }
}

fn optional_tag(tag: Option<i32>) -> String {
    tag.map_or_else(|| "default".to_string(), |tag| tag.to_string())
}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    // Not a real iOS dylib obviously. This shouldn't really be in the list of
    // dylibs if we can avoid it somehow (TODO?).
    path: "/.touchHLE/AppPickerHelpers.dylib",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

/// Be careful! These classes go in the normal class list, just like everything
/// else, so an app could try to instantiate them. Don't give them special
/// powers that could be exploited!
const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_AppPickerDelegate: NSObject

- (())iconTapped:(id)sender {
    // There is no allocWithZone: that creates AppPickerDelegateHostObject, so
    // this downcast effectively acts as an assertion that this class is being
    // used within the app picker, so it can't be abused. :)
    let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host_obj.icon_tapped = sender;
}

- (())addIpa {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).add_ipa = true;
}

- (())statusAccessoryTapped {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).status_accessory_tapped = true;
}
- (())accessPromptConfirm {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).access_prompt_confirm = true;
}
- (())accessPromptCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).access_prompt_cancel = true;
}

- (())archiveStoreClose {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_close = true;
}
- (())archiveStoreSearch {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_search = true;
}
- (())archiveStorePrevious {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_previous = true;
}
- (())archiveStoreNext {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_next = true;
}
- (())archiveStoreBack {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_back = true;
}
- (())archiveStoreRetry {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_retry = true;
}
- (())archiveStoreCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_cancel = true;
}
- (())archiveStoreItem:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_item = Some(tag);
}
- (())archiveStoreDownload:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_download = Some(tag);
}
- (())archiveStoreTab:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_tab = Some(tag);
}
- (())archiveStoreSort {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_sort = true;
}
- (())archiveStoreFuzzy {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_fuzzy = true;
}
- (())archiveStoreQueueAction:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_queue_action = Some(tag);
}
- (())archiveStoreTap:(id)sender {
    let view: id = msg![env; sender view];
    let point: CGPoint = msg![env; sender locationInView:view];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_tap = Some(point);
}
- (())textFieldShouldReturn:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    if tag == access_prompt::FIELD_TAG {
        host.access_prompt_submit = true;
    } else if !is_manual_value_tag(tag) {
        host.archive_store_search = true;
    }
    drop(host);
    let _: bool = msg![env; sender resignFirstResponder];
}
- (())textFieldDidEndEditing:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    if is_manual_value_tag(tag) {
        let text: id = msg![env; sender text];
        let value = ns_string::to_rust_string(env, text).into_owned();
        env.objc
            .borrow_mut::<AppPickerDelegateHostObject>(this)
            .manual_value = Some((tag, value));
    }
}

- (())settingsShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).settings_show = true;
}
- (())settingsHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).settings_hide = true;
}
- (())scaleHackDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_default = true;
}
- (())scaleHack1 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack1 = true;
}
- (())scaleHack2 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack2 = true;
}
- (())scaleHack3 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack3 = true;
}
- (())scaleHack4 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack4 = true;
}
- (())corruptionOff {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_mode = Some(CorruptionMode::Off);
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_mode_changed = true;
}
- (())corruptionOn {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_mode = Some(CorruptionMode::Corrupt);
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_mode_changed = true;
}
- (())corruptionStartupImmediate {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_startup_delay = Some(StartupDelayPreset::Immediate);
}
- (())corruptionStartup5 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_startup_delay = Some(StartupDelayPreset::FiveSeconds);
}
- (())corruptionStartup15 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_startup_delay = Some(StartupDelayPreset::FifteenSeconds);
}
- (())corruptionStartup30 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_startup_delay = Some(StartupDelayPreset::ThirtySeconds);
}
- (())corruptionCooldownHalf {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_cooldown = Some(CooldownPreset::HalfSecond);
}
- (())corruptionCooldown1 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_cooldown = Some(CooldownPreset::OneSecond);
}
- (())corruptionCooldown5 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_cooldown = Some(CooldownPreset::FiveSeconds);
}
- (())corruptionCooldown10 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).corruption_cooldown = Some(CooldownPreset::TenSeconds);
}
- (())gles2Off {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).gles2_mode = Some(Gles2Mode::Off);
}
- (())gles2Prefer {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).gles2_mode = Some(Gles2Mode::PreferContext);
}
- (())gles2Compatibility {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).gles2_mode = Some(Gles2Mode::Compatibility);
}
- (())orientationDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_default = true;
}
- (())orientationLandscapeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_left = true;
}
- (())orientationLandscapeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_right = true;
}
- (())orientationPortraitUpsideDown {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_portrait_upside_down = true;
}
- (())analogStickTiltControls:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).analog_stick_tilt_controls = Some(switch_state);
}
- (())network:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).network = Some(switch_state);
}
- (())gameCenterAuthenticated:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).game_center_authenticated = Some(switch_state);
}
- (())traceGLErrors:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc
        .borrow_mut::<AppPickerDelegateHostObject>(this)
        .trace_gl_errors = Some(switch_state);
}
- (())traceGLDraws:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc
        .borrow_mut::<AppPickerDelegateHostObject>(this)
        .trace_gl_draws = Some(switch_state);
}
- (())controllerToTouchDefault {
    env.objc
        .borrow_mut::<AppPickerDelegateHostObject>(this)
        .controller_to_touch = Some(None);
}
- (())controllerToTouchOn {
    env.objc
        .borrow_mut::<AppPickerDelegateHostObject>(this)
        .controller_to_touch = Some(Some(true));
}
- (())controllerToTouchOff {
    env.objc
        .borrow_mut::<AppPickerDelegateHostObject>(this)
        .controller_to_touch = Some(Some(false));
}
- (())printFPS:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).print_fps = Some(switch_state);
}
- (())popupErrors:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).popup_errors = Some(switch_state);
}
- (())vsyncAuto { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).vsync = Some(VsyncMode::Auto); }
- (())vsyncOn { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).vsync = Some(VsyncMode::On); }
- (())vsyncOff { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).vsync = Some(VsyncMode::Off); }
- (())presentModeAuto { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_mode = Some(PresentMode::Auto); }
- (())presentModeDirect { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_mode = Some(PresentMode::Direct); }
- (())presentModeReadback { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_mode = Some(PresentMode::Readback); }
- (())textureFilterDefault { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fix_texture_min_filter = Some(None); }
- (())textureFilterOn { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fix_texture_min_filter = Some(Some(true)); }
- (())textureFilterOff { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fix_texture_min_filter = Some(Some(false)); }
- (())presentRotateDefault { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_rotate = Some(None); }
- (())presentRotate0 { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_rotate = Some(Some(0)); }
- (())presentRotate90 { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_rotate = Some(Some(90)); }
- (())presentRotate180 { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_rotate = Some(Some(180)); }
- (())presentRotate270 { env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).present_rotate = Some(Some(270)); }
- (())rotationFix:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).rotation_fix = Some(switch_state);
}
- (())moviePlayerFix:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fix_movie_player = Some(switch_state);
}
- (())preserveAspectRatio:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).preserve_aspect_ratio = Some(switch_state);
}
- (())showLockScreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).show_lock_screen = Some(switch_state);
}
- (())forceComposition:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).force_composition = Some(switch_state);
}
- (())rendererMode:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).renderer_mode_tag = Some(tag as i32);
}
- (())rendererModeToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).renderer_mode_toggle = true;
}
- (())rendererModeScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).renderer_mode_scroll_to = Some(position);
    }
}
- (())overlayModeFull {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::Full);
}
- (())overlayModeMinimal {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::Minimal);
}
- (())overlayModeTitleBar {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::TitleBar);
}
- (())overlayCornerTopLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::TopLeft);
}
- (())overlayCornerTopRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::TopRight);
}
- (())overlayCornerBottomRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::BottomRight);
}
- (())overlayCornerBottomLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::BottomLeft);
}
- (())showFPS:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).show_fps = Some(switch_state);
    env.window_mut().set_show_fps_counter(switch_state);
}
- (())cheatEngine:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).cheat_engine = Some(switch_state);
}
- (())fullscreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fullscreen = Some(switch_state);
}
- (())deviceModel:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_tag = Some(tag as i32);
}
- (())deviceModelToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_toggle = true;
}
- (())deviceModelScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_scroll_to = Some(position);
    }
}
- (())language:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_tag = Some(tag as i32);
}
- (())languageToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_toggle = true;
}
- (())languageScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_scroll_to = Some(position);
    }
}
- (())iconTouchDown:(id)sender { // UIButton*
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).icon_touch_down = sender;
}
- (())iconTouchCancelled:(id)_sender { // UIButton*
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).icon_touch_cancelled = true;
}
- (())deleteCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).delete_cancel = true;
}
- (())resetSettingsTapped {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).settings_reset = true;
}
- (())deleteConfirm {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).delete_confirm = true;
}
- (())resetSettingsConfirm {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).reset_confirm = true;
}
- (())saveErrorDismiss {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).save_error_dismiss = true;
}
- (())swipeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).swipe_left = true;
}
- (())swipeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).swipe_right = true;
}
- (())lockSliderChanged:(id)sender {
    let progress: f32 = msg![env; sender value]; // UISlider*
    let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host.lock_slider_progress = progress;
    if progress >= 0.96 {
        host.lock_screen_unlocked = true;
    }
}
- (())openRecentApp {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).recent_app_tapped = true;
}

@end

};

fn show_app_picker_gui(
    mut options: Options,
    apps: Result<Vec<AppInfo>, String>,
    show_lock_screen: bool,
) -> Result<(PathBuf, Vec<String>), String> {
    if std::env::consts::OS == "ios" && options.scale_hack.get() == 1 {
        options.scale_hack = NonZeroU32::new(3).unwrap();
    }

    let icon = {
        let bytes: &[u8] = match crate::branding() {
            "" | "UNOFFICIAL" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png"))
            }
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => panic!(),
        };
        let mut image = Image::from_bytes(bytes).unwrap();
        // Should match Bundle::load_icon(): the corner radius scales with the
        // icon's resolution.
        image.round_icon_corners(/* add_sheen: */ true);
        image
    };
    let environment = Environment::new_without_app(options, icon)?;
    Ok(environment.run_app_picker(move |env| app_picker_inner(env, apps, show_lock_screen)))
}

fn app_picker_inner(
    env: &mut Environment,
    mut apps: Result<Vec<AppInfo>, String>,
    show_lock_screen: bool,
) -> (PathBuf, Vec<String>) {
    let mut option_args = Vec::new();
    // Note that objects are generally not released in this code, because they
    // don't need to be: the entire Environment is thrown away at the end.

    // Bypassing UIApplicationMain!
    let ui_application: id = msg_class![env; UIApplication new];
    let delegate = env
        .objc
        .get_known_class("_touchHLE_AppPickerDelegate", &mut env.mem);
    let delegate = env.objc.alloc_object(
        delegate,
        Box::<AppPickerDelegateHostObject>::default(),
        &mut env.mem,
    );
    () = msg![env; ui_application setDelegate:delegate];

    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];

    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];

    let app_frame: CGRect = msg![env; screen applicationFrame];

    // Wallpaper
    let mut found_wallpaper = false;
    let mut wallpaper_view: Option<id> = None;
    for candidate in paths::WALLPAPER_FILES {
        let candidate = paths::user_data_base_path().join(candidate);
        if !candidate.exists() {
            continue;
        }
        found_wallpaper = true;

        let image = match std::fs::read(&candidate) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't read {}: {}", candidate.display(), e);
                break;
            }
        };
        let image = match Image::from_bytes(&image) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't decode {}: {}", candidate.display(), e);
                break;
            }
        };

        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        let wallpaper: id = msg_class![env; UIImageView alloc];
        let wallpaper: id = msg![env; wallpaper initWithImage:image];
        // The wallpaper also covers the status bar strip above the
        // application frame, like the old iOS home screen.
        let status_offset = app_frame.origin.y;
        () = msg![env; wallpaper setFrame:(CGRect {
            origin: CGPoint {
                x: 0.0,
                y: 0.0,
            },
            size: CGSize {
                width: app_frame.size.width,
                height: app_frame.size.height + status_offset,
            },
        })];
        () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
        () = msg![env; window addSubview:wallpaper];
        wallpaper_view = Some(wallpaper);
        break;
    }
    if !found_wallpaper {
        let CGSize { width, height } = app_frame.size;
        log!(
            "No wallpaper found; filename can be one of: {}; ideal size is {}×{} pixels",
            paths::WALLPAPER_FILES.join(", "),
            width,
            height,
        );
    }

    // Old-iOS status bar, drawn in the strip above the application frame. It
    // sits above the wallpaper, while the (transparent) main view is added
    // after it: the home screen shows the bar through the main view, but the
    // opaque Settings screen covers it when open.
    let (status_bar, status_time_label, status_charge_view) = if app_frame.origin.y >= 12.0 {
        let (bar, time, charge) =
            make_status_bar(env, delegate, bounds.size.width, app_frame.origin.y);
        () = msg![env; window addSubview:bar];
        // The level is cached at window creation, so reading it here is safe
        // even on Android (no JNI on this coroutine stack).
        update_battery_indicator(env, charge, crate::window::cached_battery_status());
        (Some(bar), Some(time), Some(charge))
    } else {
        (None, None, None)
    };

    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:app_frame];
    () = msg![env; window addSubview:main_view];

    // Old-iOS home screen paging: swipe left/right anywhere on the home screen.
    // The recognizers are attached to the main view, so touches that land on
    // icons or on the dock are seen too.
    let swipe_left_selector = env.objc.lookup_selector("swipeLeft").unwrap();
    let swipe_right_selector = env.objc.lookup_selector("swipeRight").unwrap();
    let swipe_left: id = msg_class![env; UISwipeGestureRecognizer alloc];
    let swipe_left: id = msg![env;
        swipe_left initWithTarget:delegate action:swipe_left_selector];
    () = msg![env; swipe_left setDirection:UISwipeGestureRecognizerDirectionLeft];
    () = msg![env; main_view addGestureRecognizer:swipe_left];
    let swipe_right: id = msg_class![env; UISwipeGestureRecognizer alloc];
    let swipe_right: id = msg![env;
        swipe_right initWithTarget:delegate action:swipe_right_selector];
    () = msg![env; swipe_right setDirection:UISwipeGestureRecognizerDirectionRight];
    () = msg![env; main_view addGestureRecognizer:swipe_right];

    // Resolve the last launched app for the dock shortcut. The record is a
    // path under the apps directory, and is ignored if that app was removed.
    let remembered_app_path = read_last_app_path();
    let recent_app = apps.as_mut().ok().and_then(|apps| {
        let app = apps.iter_mut().find(|app| {
            remembered_app_path
                .as_ref()
                .is_some_and(|path| path == &app.path)
        })?;
        let image = if let Some(icon) = app.icon.as_ref() {
            let cg_image = cg_image::from_image(env, icon.clone());
            msg_class![env; UIImage imageWithCGImage:cg_image]
        } else {
            make_icon_from_text(
                env,
                &app.display_name.chars().take(2).collect::<String>(),
                28.0,
                0.0,
                (0.35, 0.35, 0.35, 1.0),
            )
        };
        Some((app.path.clone(), app.display_name.clone(), image))
    });
    let mut icon_grid_stuff = match &mut apps {
        Ok(ref mut apps) => {
            let mut icon_grid_stuff =
                make_icon_grid(env, delegate, main_view, app_frame, apps.len());
            update_icon_grid(env, &mut icon_grid_stuff, apps, 0, 0);
            Some(icon_grid_stuff)
        }
        Err(e) => {
            let label_frame = CGRect {
                origin: CGPoint { x: 10.0, y: 10.0 },
                size: CGSize {
                    width: app_frame.size.width - 20.0,
                    height: app_picker_grid_bottom(app_frame.size.height) - 20.0,
                },
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            let text = ns_string::from_rust_string(env, e.clone());
            () = msg![env; label setText:text];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            () = msg![env; label setNumberOfLines:0]; // unlimited
            let text_color: id = msg_class![env; UIColor lightGrayColor];
            () = msg![env; label setTextColor:text_color];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; main_view addSubview:label];
            None
        }
    };

    // Old-iOS page dots, and the dock with the shortcuts that aren't apps.
    let mut page_dots = Vec::new();
    if let Some(ref grid) = icon_grid_stuff {
        page_dots = make_page_dots(env, main_view, app_frame.size, grid.pages.len());
        update_page_dots(env, &page_dots, 0);
    }
    let dock = make_dock(
        env,
        delegate,
        main_view,
        app_frame.size,
        recent_app
            .as_ref()
            .map(|(_, name, image)| (name.as_str(), *image)),
    );
    // Build stamp on the page-dot row, right-aligned like the one in the
    // reference home screen. It is added after the dock so nothing covers it.
    let version_label = {
        let label_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: app_picker_dots_top(app_frame.size.height) - 2.0,
            },
            size: CGSize {
                width: app_frame.size.width - 8.0,
                height: 12.0,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::from_rust_string(
            env,
            format!("{} ({})", crate::DISPLAY_NAME, crate::COMMIT_HASH),
        );
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentRight];
        let font_size: CGFloat = 10.0;
        let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
        () = msg![env; label setFont:font];
        let text_color: id = msg_class![env; UIColor lightGrayColor];
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);
        () = msg![env; label setShadowColor:shadow];
        () = msg![env; label setShadowOffset:(CGSize {
            width: 0.0,
            height: 1.0,
        })];
        () = msg![env; main_view addSubview:label];
        label
    };

    let angle_backend_available = crate::window::angle_backend_available();
    let mut picker_settings = PickerSettings::load(&env.options, angle_backend_available);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    env.window_mut()
        .set_preserve_aspect_ratio(picker_settings.preserve_aspect_ratio);
    let renderer_mode_dropdown_enabled = renderer_mode_dropdown_enabled();
    let show_fullscreen_row = !crate::window::Window::rotatable_fullscreen();
    let overlay_hidden_by_environment = crate::gles::perf_overlay::hidden_by_environment();
    let initial_quick_options_show_fps = picker_settings.show_fps && !overlay_hidden_by_environment;
    env.window_mut().set_performance_overlay_settings(
        initial_quick_options_show_fps,
        picker_settings.overlay_mode,
        picker_settings.overlay_corner,
    );
    let settings = setup_settings(
        env,
        delegate,
        main_view,
        app_frame,
        &picker_settings,
        renderer_mode_dropdown_enabled,
        initial_quick_options_show_fps,
        show_fullscreen_row,
    );
    let mut quick_options_show_fps = initial_quick_options_show_fps;
    let mut device_state = DropdownState {
        selected: picker_settings.device_model_tag,
        ..Default::default()
    };
    let mut renderer_state = DropdownState {
        selected: Some(renderer_mode_tag(picker_settings.renderer_mode)),
        ..Default::default()
    };
    let mut language_state = DropdownState {
        selected: picker_settings.language_tag,
        ..Default::default()
    };
    let mut last_saved_picker_settings = picker_settings.clone();
    let mut next_settings_save = Instant::now();

    update_scale_hack_buttons(
        env,
        &settings.scale_hack_buttons,
        picker_settings.scale_hack,
    );
    update_orientation_buttons(
        env,
        &settings.orientation_buttons,
        picker_settings.orientation,
    );
    update_performance_overlay_buttons(
        env,
        &settings.overlay_mode_buttons,
        &settings.overlay_corner_buttons,
        picker_settings.overlay_mode,
        picker_settings.overlay_corner,
    );
    update_segment_buttons(
        env,
        &settings.corruption_mode_buttons,
        picker_settings.corruption_mode.selected_index() as usize,
    );
    update_segment_buttons(
        env,
        &settings.corruption_startup_delay_buttons,
        picker_settings.corruption_startup_delay.selected_index(),
    );
    update_segment_buttons(
        env,
        &settings.corruption_cooldown_buttons,
        picker_settings.corruption_cooldown.selected_index(),
    );
    update_segment_buttons(
        env,
        &settings.gles2_mode_buttons,
        picker_settings.gles2_mode.selected_index() as usize,
    );
    update_segment_buttons(
        env,
        &settings.vsync_buttons,
        vsync_selected_index(picker_settings.vsync),
    );
    update_segment_buttons(
        env,
        &settings.present_mode_buttons,
        present_mode_selected_index(picker_settings.present_mode),
    );
    update_segment_buttons(
        env,
        &settings.texture_filter_buttons,
        texture_filter_selected_index(picker_settings.fix_texture_min_filter),
    );
    update_segment_buttons(
        env,
        &settings.present_rotate_buttons,
        present_rotate_selected_index(picker_settings.present_rotate),
    );
    update_segment_buttons(
        env,
        &settings.controller_to_touch_buttons,
        controller_to_touch_selected_index(picker_settings.controller_to_touch),
    );
    let (touch_screen_width, touch_screen_height) =
        picker_touch_screen_size(&env.options, &picker_settings);
    refresh_manual_value_fields(
        env,
        &settings.manual_value_fields,
        touch_screen_width,
        touch_screen_height,
        &mut picker_settings,
    );
    refresh_dropdown(
        env,
        &settings.device_dropdown,
        &device_model_label_for_tag(device_state.selected),
        &device_state,
    );
    refresh_dropdown(
        env,
        &settings.language_dropdown,
        &language_label_for_tag(language_state.selected),
        &language_state,
    );
    refresh_dropdown(
        env,
        &settings.renderer_dropdown,
        &renderer_mode_label_for_tag(renderer_state.selected),
        &renderer_state,
    );

    // Built after the settings screen, so that it is drawn on top of it.
    let delete_dialog = make_delete_dialog(
        env,
        delegate,
        main_view,
        app_frame.size,
        "deleteCancel",
        "deleteConfirm",
    );
    let reset_dialog = make_delete_dialog(
        env,
        delegate,
        main_view,
        app_frame.size,
        "deleteCancel",
        "resetSettingsConfirm",
    );
    let save_error_dialog = make_delete_dialog(
        env,
        delegate,
        main_view,
        app_frame.size,
        "saveErrorDismiss",
        "saveErrorDismiss",
    );

    () = msg![env; window makeKeyAndVisible];

    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);
    let mut current_page: usize = 0;
    // Discard arrow presses made before the picker was shown.
    PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
    PENDING_MOUSE_WHEEL.store(0, Ordering::Relaxed);
    // If the user taps the dock's "Add .ipa" shortcut, this records the .ipa
    // files that existed at that moment; once a new one shows up, the app list
    // is re-enumerated.
    let mut awaited_ipa: Option<IpaWatch> = None;
    // The app icon under a finger that may turn into a long press, and when
    // the finger went down.
    let mut press: Option<(id, Instant)> = None;
    // The icon of the last long press. The release that ends it is not a tap.
    let mut long_pressed_icon: Option<id> = None;
    // Index of the app whose delete dialog is open, if any.
    let mut pending_delete: Option<usize> = None;

    // The status bar clock and battery only need refreshing now and then.
    let mut status_time = status_bar_time_string();
    let mut status_time_updated_at = Instant::now();
    let mut status_battery_updated_at = Instant::now();

    let mut chrome_views: Vec<id> = Vec::with_capacity(3 + page_dots.len());
    if let Some(bar) = status_bar {
        chrome_views.push(bar);
    }
    chrome_views.push(dock);
    chrome_views.push(version_label);
    chrome_views.extend(page_dots.iter().copied());

    let main_run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    if show_lock_screen && picker_settings.show_lock_screen {
        run_lock_screen(
            env,
            main_run_loop,
            delegate,
            main_view,
            app_frame.size,
            icon_grid_stuff.as_ref(),
            status_bar,
            status_time_label,
            dock,
            version_label,
            &page_dots,
        );
        if let Some(ref grid) = icon_grid_stuff {
            play_home_screen_unlock_animation(
                env,
                main_run_loop,
                delegate,
                grid,
                app_frame.size,
                wallpaper_view,
                &chrome_views,
            );
            PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
        }
    }
    // If an app is picked, this loop returns. If the user quits touchHLE, the
    // process exits.
    let (app_path, app_id) = loop {
        run_run_loop_single_iteration(env, main_run_loop);
        update_settings_scrollbar(env, &settings);
        sync_dropdown_scroll(env, &settings.device_dropdown, &mut device_state);
        sync_dropdown_scroll(env, &settings.language_dropdown, &mut language_state);
        sync_dropdown_scroll(env, &settings.renderer_dropdown, &mut renderer_state);

        if status_time_updated_at.elapsed() >= Duration::from_secs(10) {
            status_time_updated_at = Instant::now();
            let now = status_bar_time_string();
            if now != status_time {
                status_time = now;
                if let Some(label) = status_time_label {
                    let text = ns_string::from_rust_string(env, status_time.clone());
                    () = msg![env; label setText:text];
                    release(env, text);
                }
            }
        }

        // Keep the battery indicator in sync with the real battery level. The
        // refresh itself must happen on the SDL thread on Android (SDL's
        // battery query uses JNI), and the picker runs on a coroutine stack.
        if let Some(charge) = status_charge_view {
            if status_battery_updated_at.elapsed() >= Duration::from_secs(30) {
                status_battery_updated_at = Instant::now();
                if env.window.is_some() {
                    env.on_parent_stack_in_coroutine(|_, _| crate::window::refresh_battery_cache());
                }
                update_battery_indicator(env, charge, crate::window::cached_battery_status());
            }
        }

        // Left/right arrow keys switch pages, like a swipe does.
        let arrow = PENDING_ARROW_KEY.swap(0, Ordering::Relaxed);
        if arrow != 0 {
            if let Some(grid) = icon_grid_stuff.as_mut() {
                let settings_view = settings.main_view;
                let settings_open: bool = !msg![env; settings_view isHidden];
                let new_page = if arrow > 0 {
                    current_page + 1
                } else {
                    current_page.wrapping_sub(1)
                };
                if !settings_open && pending_delete.is_none() && new_page < grid.pages.len() {
                    slide_to_page(
                        env,
                        main_run_loop,
                        delegate,
                        grid,
                        apps.as_mut().unwrap(),
                        new_page,
                        arrow as CGFloat,
                    );
                    current_page = new_page;
                    update_page_dots(env, &page_dots, current_page);
                }
            }
        }

        // Swipes are recognized when the finger lifts, so they can end on an
        // icon without opening it.
        let (swipe_left, swipe_right) = {
            let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
            (
                std::mem::take(&mut host_obj.swipe_left),
                std::mem::take(&mut host_obj.swipe_right),
            )
        };
        if swipe_left || swipe_right {
            {
                let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
                host_obj.icon_tapped = nil;
                host_obj.icon_touch_down = nil;
                host_obj.icon_touch_cancelled = false;
                host_obj.add_ipa = false;
                host_obj.settings_show = false;
                host_obj.settings_hide = false;
            }
            press = None;
            long_pressed_icon = None;
            if let Some(grid) = icon_grid_stuff.as_mut() {
                let settings_view = settings.main_view;
                let settings_open: bool = !msg![env; settings_view isHidden];
                let new_page = if swipe_left {
                    current_page + 1
                } else {
                    current_page.saturating_sub(1)
                };
                if !settings_open
                    && pending_delete.is_none()
                    && new_page < grid.pages.len()
                    && new_page != current_page
                {
                    slide_to_page(
                        env,
                        main_run_loop,
                        delegate,
                        grid,
                        apps.as_mut().unwrap(),
                        new_page,
                        if swipe_left { 1.0 } else { -1.0 },
                    );
                    current_page = new_page;
                    update_page_dots(env, &page_dots, current_page);
                }
            }
            continue;
        }

        let (touch_down_icon, touch_cancelled, icon_tapped) = {
            let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
            (
                std::mem::take(&mut host_obj.icon_touch_down),
                std::mem::take(&mut host_obj.icon_touch_cancelled),
                std::mem::take(&mut host_obj.icon_tapped),
            )
        };
        if touch_down_icon != nil {
            press = Some((touch_down_icon, Instant::now()));
            long_pressed_icon = None;
        }
        if touch_cancelled {
            press = None;
        }
        // Holding an app icon long enough opens its delete dialog, as in the
        // iOS icon menu. The dialog is shown while the finger is still down.
        if let Some((button, pressed_at)) = press {
            if pressed_at.elapsed() >= APP_LONG_PRESS_DURATION {
                press = None;
                let app_idx = icon_grid_stuff
                    .as_ref()
                    .and_then(|grid| grid.icon_map.get(&button).copied());
                let name = app_idx.and_then(|app_idx| {
                    apps.as_ref()
                        .ok()
                        .and_then(|list| list.get(app_idx))
                        .map(|app| app.display_name.clone())
                });
                if let (Some(app_idx), Some(name)) = (app_idx, name) {
                    long_pressed_icon = Some(button);
                    pending_delete = Some(app_idx);
                    set_dialog_content(
                        env,
                        &delete_dialog,
                        &format!("Delete “{name}”?"),
                        "This will delete the app and all of its saved data.",
                        Some("Delete"),
                    );
                    show_delete_dialog(env, main_run_loop, delegate, &delete_dialog);
                }
            }
        }
        if icon_tapped != nil {
            press = None;
            // The release that ends a long press is not a tap.
            if long_pressed_icon.take() == Some(icon_tapped) {
                continue;
            }
            match icon_grid_stuff
                .as_ref()
                .unwrap()
                .icon_map
                .get(&icon_tapped)
                .copied()
            {
                Some(app_idx) => {
                    let app = &apps.as_ref().unwrap()[app_idx];
                    let app_path = app.path.clone();
                    let app_id = app.bundle_id.clone();
                    remember_last_app_path(&app_path);
                    echo!("Picked: {}", app_path.display());
                    // iOS-style launch: the icon travels to the middle of the
                    // screen while the screen fades to black.
                    let icon_image: id = msg![env; icon_tapped currentImage];
                    let icon_frame: CGRect = msg![env; icon_tapped frame];
                    play_app_launch_animation(
                        env,
                        main_run_loop,
                        delegate,
                        main_view,
                        app_frame.size,
                        icon_frame,
                        icon_image,
                    );
                    break (app_path, app_id);
                }
                None => (), // Tapped on a black space
            }
            continue;
        }
        let status_accessory_tapped = {
            let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
            std::mem::take(&mut host.status_accessory_tapped)
        };
        if status_accessory_tapped {
            press = None;
            long_pressed_icon = None;
            let downloaded = access_prompt::run_access_prompt(
                env,
                main_run_loop,
                delegate,
                main_view,
                app_frame,
            ) && archive_store::run_archive_store(
                env,
                main_run_loop,
                delegate,
                main_view,
                app_frame,
                &apps_dir,
            );
            // Both screens use nested event loops. Do not let their touches or
            // swipes leak back into the home screen.
            {
                let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
                host.icon_tapped = nil;
                host.icon_touch_down = nil;
                host.icon_touch_cancelled = false;
                host.swipe_left = false;
                host.swipe_right = false;
                host.status_accessory_tapped = false;
            }
            PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
            press = None;
            long_pressed_icon = None;
            if downloaded {
                awaited_ipa = None;
                if let Some(grid) = icon_grid_stuff.as_mut() {
                    let page_count =
                        reload_app_grid(env, grid, &mut apps, &mut current_page, &apps_dir);
                    rebuild_page_dots(env, main_view, app_frame.size, &mut page_dots, page_count);
                    update_page_dots(env, &page_dots, current_page);
                }
            }
            continue;
        }

        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        let recent_app_tapped = std::mem::take(&mut host_obj.recent_app_tapped);
        let add_ipa = std::mem::take(&mut host_obj.add_ipa);
        // Read every flag the settings screen set here, in one go.
        let reset_request = if std::mem::take(&mut host_obj.settings_reset) {
            ResetSettingsRequest::Ask
        } else if std::mem::take(&mut host_obj.reset_confirm) {
            ResetSettingsRequest::Confirm
        } else if std::mem::take(&mut host_obj.save_error_dismiss) {
            ResetSettingsRequest::DismissError
        } else {
            ResetSettingsRequest::None
        };
        // The reset handlers below need `env`, so the borrow ends here and is
        // taken again for the handlers that still need `host_obj`.
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        if recent_app_tapped {
            if let Some((path, _, _)) = recent_app.as_ref() {
                echo!("Picked last launched app: {}", path.display());
                let app_id = BundleData::open_any(path)
                    .and_then(|data| Bundle::new_bundle_and_fs_from_host_path(data, true))
                    .map(|(bundle, _)| bundle.bundle_identifier().to_owned());
                let app_id = match app_id {
                    Ok(app_id) if !app_id.is_empty() => app_id,
                    Ok(_) | Err(_) => {
                        log!("Warning: Couldn't inspect the last app's bundle identifier; some app-specific options may not be applied");
                        String::new()
                    }
                };
                break (path.clone(), app_id);
            }
        }
        if add_ipa {
            // Snapshot the .ipa files that exist right now, so that when the
            // system file picker finishes, we can detect the new file and
            // refresh the app grid.
            awaited_ipa = Some(IpaWatch {
                last_seen: list_top_level_ipa_files(&apps_dir),
                dirty: false,
                last_change: None,
            });
            // MainActivity (Android) opens the system file picker and copies
            // the picked file into the apps directory. On other platforms,
            // the apps directory is opened in the file manager instead.
            if let Err(e) = crate::window::launch_ipa_picker(env) {
                echo!("Couldn't open IPA picker: {}", e);
            }
        } else if std::mem::take(&mut host_obj.settings_show) {
            show_settings_screen(env, main_run_loop, delegate, &settings);
        } else if std::mem::take(&mut host_obj.settings_hide) {
            // Commit an active manual field before leaving Settings; otherwise
            // tapping Done could hide the field before UIKit ends editing.
            drop(host_obj);
            for &(_, field, _) in &settings.manual_value_fields {
                let _: bool = msg![env; field resignFirstResponder];
            }
            if let Some((tag, text)) = {
                let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
                std::mem::take(&mut host_obj.manual_value)
            } {
                let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
                apply_manual_value(&mut picker_settings, tag, &text, width, height);
                refresh_manual_value_fields(
                    env,
                    &settings.manual_value_fields,
                    width,
                    height,
                    &mut picker_settings,
                );
            }
            if renderer_state.open {
                renderer_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.renderer_dropdown,
                    &renderer_mode_label_for_tag(renderer_state.selected),
                    &renderer_state,
                );
            }
            if device_state.open {
                device_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.device_dropdown,
                    &device_model_label_for_tag(device_state.selected),
                    &device_state,
                );
            }
            if language_state.open {
                language_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.language_dropdown,
                    &language_label_for_tag(language_state.selected),
                    &language_state,
                );
            }
            hide_settings_screen(env, main_run_loop, delegate, &settings);
        } else if std::mem::take(&mut host_obj.scale_hack_default) {
            picker_settings.scale_hack = None;
            update_scale_hack_buttons(
                env,
                &settings.scale_hack_buttons,
                picker_settings.scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack1) {
            picker_settings.scale_hack = NonZeroU32::new(1);
            update_scale_hack_buttons(
                env,
                &settings.scale_hack_buttons,
                picker_settings.scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack2) {
            picker_settings.scale_hack = NonZeroU32::new(2);
            update_scale_hack_buttons(
                env,
                &settings.scale_hack_buttons,
                picker_settings.scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack3) {
            picker_settings.scale_hack = NonZeroU32::new(3);
            update_scale_hack_buttons(
                env,
                &settings.scale_hack_buttons,
                picker_settings.scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack4) {
            picker_settings.scale_hack = NonZeroU32::new(4);
            update_scale_hack_buttons(
                env,
                &settings.scale_hack_buttons,
                picker_settings.scale_hack,
            );
        } else if std::mem::take(&mut host_obj.orientation_default) {
            picker_settings.orientation = None;
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                picker_settings.orientation,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_left) {
            picker_settings.orientation = Some(DeviceOrientation::LandscapeLeft);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                picker_settings.orientation,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_right) {
            picker_settings.orientation = Some(DeviceOrientation::LandscapeRight);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                picker_settings.orientation,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if std::mem::take(&mut host_obj.orientation_portrait_upside_down) {
            picker_settings.orientation = Some(DeviceOrientation::PortraitUpsideDown);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                picker_settings.orientation,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.device_model_tag) {
            device_state.selected = Some(tag);
            picker_settings.device_model_tag = device_state.selected;
            device_state.open = false;
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if std::mem::take(&mut host_obj.device_model_toggle) {
            device_state.open = !device_state.open;
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.device_model_scroll_to) {
            device_state.scroll =
                dropdown_scroll_at(position, settings.device_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.renderer_mode_tag) {
            renderer_state.selected = Some(tag);
            renderer_state.open = false;
            picker_settings.renderer_mode = renderer_mode_from_tag(Some(tag));
            refresh_dropdown(
                env,
                &settings.renderer_dropdown,
                &renderer_mode_label_for_tag(renderer_state.selected),
                &renderer_state,
            );
        } else if std::mem::take(&mut host_obj.renderer_mode_toggle) {
            renderer_state.open = !renderer_state.open;
            refresh_dropdown(
                env,
                &settings.renderer_dropdown,
                &renderer_mode_label_for_tag(renderer_state.selected),
                &renderer_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.renderer_mode_scroll_to) {
            renderer_state.scroll =
                dropdown_scroll_at(position, settings.renderer_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.renderer_dropdown,
                &renderer_mode_label_for_tag(renderer_state.selected),
                &renderer_state,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.language_tag) {
            language_state.selected = Some(tag);
            language_state.open = false;
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if std::mem::take(&mut host_obj.language_toggle) {
            language_state.open = !language_state.open;
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.language_scroll_to) {
            language_state.scroll =
                dropdown_scroll_at(position, settings.language_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if std::mem::take(&mut host_obj.delete_cancel) {
            pending_delete = None;
            hide_delete_dialog(env, main_run_loop, delegate, &delete_dialog);
        } else if std::mem::take(&mut host_obj.delete_confirm) {
            if let Some(app_idx) = pending_delete.take() {
                hide_delete_dialog(env, main_run_loop, delegate, &delete_dialog);
                let result = match apps.as_ref().ok().and_then(|list| {
                    list.get(app_idx)
                        .map(|app| delete_app(app, list.as_slice()))
                }) {
                    Some(res) => res,
                    None => Err("the app is no longer in the list".to_string()),
                };
                match result {
                    Ok(()) => {
                        if let Some(grid) = icon_grid_stuff.as_mut() {
                            let page_count =
                                reload_app_grid(env, grid, &mut apps, &mut current_page, &apps_dir);
                            rebuild_page_dots(
                                env,
                                main_view,
                                app_frame.size,
                                &mut page_dots,
                                page_count,
                            );
                            update_page_dots(env, &page_dots, current_page);
                        }
                    }
                    Err(e) => echo!("Couldn't delete app: {}", e),
                }
            }
        } else if let Some(mode) = std::mem::take(&mut host_obj.vsync) {
            picker_settings.vsync = mode;
            update_segment_buttons(env, &settings.vsync_buttons, vsync_selected_index(mode));
        } else if let Some(mode) = std::mem::take(&mut host_obj.present_mode) {
            picker_settings.present_mode = mode;
            update_segment_buttons(
                env,
                &settings.present_mode_buttons,
                present_mode_selected_index(mode),
            );
        } else if let Some(value) = std::mem::take(&mut host_obj.fix_texture_min_filter) {
            picker_settings.fix_texture_min_filter = value;
            update_segment_buttons(
                env,
                &settings.texture_filter_buttons,
                texture_filter_selected_index(value),
            );
        } else if let Some(value) = std::mem::take(&mut host_obj.present_rotate) {
            picker_settings.present_rotate = value;
            update_segment_buttons(
                env,
                &settings.present_rotate_buttons,
                present_rotate_selected_index(value),
            );
        } else if let Some(enabled) = std::mem::take(&mut host_obj.print_fps) {
            picker_settings.print_fps = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.popup_errors) {
            picker_settings.popup_errors = enabled;
        } else if let Some(mode) = std::mem::take(&mut host_obj.controller_to_touch) {
            picker_settings.controller_to_touch = mode;
            update_segment_buttons(
                env,
                &settings.controller_to_touch_buttons,
                controller_to_touch_selected_index(mode),
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if let Some((tag, text)) = std::mem::take(&mut host_obj.manual_value) {
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            apply_manual_value(&mut picker_settings, tag, &text, width, height);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if let Some(enabled) = std::mem::take(&mut host_obj.analog_stick_tilt_controls) {
            picker_settings.analog_stick_tilt_controls = enabled;
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if let Some(enabled) = std::mem::take(&mut host_obj.network) {
            picker_settings.network = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.game_center_authenticated) {
            picker_settings.game_center_authenticated = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.cheat_engine) {
            picker_settings.cheat_engine = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.show_fps) {
            quick_options_show_fps = enabled;
            picker_settings.show_fps = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.trace_gl_errors) {
            picker_settings.trace_gl_errors = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.trace_gl_draws) {
            picker_settings.trace_gl_draws = enabled;
        } else if let Some(mode) = std::mem::take(&mut host_obj.overlay_mode) {
            picker_settings.overlay_mode = mode;
            update_performance_overlay_buttons(
                env,
                &settings.overlay_mode_buttons,
                &settings.overlay_corner_buttons,
                picker_settings.overlay_mode,
                picker_settings.overlay_corner,
            );
            env.window_mut().set_performance_overlay_settings(
                quick_options_show_fps,
                picker_settings.overlay_mode,
                picker_settings.overlay_corner,
            );
        } else if let Some(corner) = std::mem::take(&mut host_obj.overlay_corner) {
            picker_settings.overlay_corner = corner;
            update_performance_overlay_buttons(
                env,
                &settings.overlay_mode_buttons,
                &settings.overlay_corner_buttons,
                picker_settings.overlay_mode,
                picker_settings.overlay_corner,
            );
            env.window_mut().set_performance_overlay_settings(
                quick_options_show_fps,
                picker_settings.overlay_mode,
                picker_settings.overlay_corner,
            );
        } else if let Some(rotation_fix) = std::mem::take(&mut host_obj.rotation_fix) {
            picker_settings.rotation_fix = Some(rotation_fix);
        } else if let Some(fix_movie_player) = std::mem::take(&mut host_obj.fix_movie_player) {
            picker_settings.fix_movie_player = Some(fix_movie_player);
        } else if std::mem::take(&mut host_obj.corruption_mode_changed) {
            picker_settings.corruption_mode =
                host_obj.corruption_mode.unwrap_or(CorruptionMode::Off);
            update_segment_buttons(
                env,
                &settings.corruption_mode_buttons,
                picker_settings.corruption_mode.selected_index() as usize,
            );
            let (width, height) = picker_touch_screen_size(&env.options, &picker_settings);
            refresh_manual_value_fields(
                env,
                &settings.manual_value_fields,
                width,
                height,
                &mut picker_settings,
            );
        } else if let Some(preset) = std::mem::take(&mut host_obj.corruption_startup_delay) {
            picker_settings.corruption_startup_delay = preset;
            update_segment_buttons(
                env,
                &settings.corruption_startup_delay_buttons,
                preset.selected_index(),
            );
        } else if let Some(preset) = std::mem::take(&mut host_obj.corruption_cooldown) {
            picker_settings.corruption_cooldown = preset;
            update_segment_buttons(
                env,
                &settings.corruption_cooldown_buttons,
                preset.selected_index(),
            );
        } else if let Some(mode) = std::mem::take(&mut host_obj.gles2_mode) {
            picker_settings.gles2_mode = mode;
            update_segment_buttons(
                env,
                &settings.gles2_mode_buttons,
                picker_settings.gles2_mode.selected_index() as usize,
            );
        } else if let Some(preserve_aspect_ratio) =
            std::mem::take(&mut host_obj.preserve_aspect_ratio)
        {
            picker_settings.preserve_aspect_ratio = preserve_aspect_ratio;
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            env.window_mut()
                .set_preserve_aspect_ratio(preserve_aspect_ratio);
        } else if let Some(enabled) = std::mem::take(&mut host_obj.show_lock_screen) {
            picker_settings.show_lock_screen = enabled;
        } else if let Some(force_composition) = std::mem::take(&mut host_obj.force_composition) {
            picker_settings.force_composition = force_composition;
        } else if let Some(fullscreen) = std::mem::take(&mut host_obj.fullscreen) {
            picker_settings.fullscreen = fullscreen;
        }

        let wheel = PENDING_MOUSE_WHEEL.swap(0, Ordering::Relaxed);
        if wheel != 0 {
            let language_open = language_state.open;
            let dropdown = if language_open {
                Some((&mut language_state, &settings.language_dropdown))
            } else if device_state.open {
                Some((&mut device_state, &settings.device_dropdown))
            } else {
                None
            };
            if let Some((state, dropdown)) = dropdown {
                state.scroll = (state.scroll - wheel as isize)
                    .clamp(0, dropdown_max_scroll(dropdown.items.len()));
                let label = if language_open {
                    language_label_for_tag(state.selected)
                } else {
                    device_model_label_for_tag(state.selected)
                };
                refresh_dropdown(env, dropdown, &label, state);
            } else if !msg![env; (settings.main_view) isHidden] {
                let scroll = settings.scroll_view;
                let offset: CGPoint = msg![env; scroll contentOffset];
                let bounds: CGRect = msg![env; scroll bounds];
                let content_size: CGSize = msg![env; scroll contentSize];
                let max_y = (content_size.height - bounds.size.height).max(0.0);
                let new_y = (offset.y - wheel as CGFloat * 40.0).clamp(0.0, max_y);
                if new_y != offset.y {
                    () = msg![env; scroll setContentOffset:(CGPoint { x: offset.x, y: new_y })];
                }
            }
        }

        picker_settings.device_model_tag = device_state.selected;
        picker_settings.language_tag = language_state.selected;

        let mut drafts = SettingsDrafts {
            picker_settings: picker_settings.clone(),
            last_saved_picker_settings: last_saved_picker_settings.clone(),
            device_state: device_state.clone(),
            language_state: language_state.clone(),
            renderer_state: renderer_state.clone(),
        };
        handle_reset_settings_request(
            env,
            reset_request,
            main_run_loop,
            delegate,
            (&settings, &reset_dialog, &save_error_dialog, &mut drafts),
        );
        picker_settings = drafts.picker_settings;
        last_saved_picker_settings = drafts.last_saved_picker_settings;
        device_state = drafts.device_state;
        language_state = drafts.language_state;
        renderer_state = drafts.renderer_state;

        if is_dialog_visible(env, &reset_dialog) || is_dialog_visible(env, &save_error_dialog) {
            // While a confirmation or error dialog is up, only the dialog's
            // own buttons may change the settings.
        } else if picker_settings != last_saved_picker_settings
            && Instant::now() >= next_settings_save
        {
            match picker_settings.save() {
                Ok(()) => last_saved_picker_settings = picker_settings.clone(),
                Err(error) => {
                    log!("Warning: couldn't save app picker settings: {error}");
                    next_settings_save = Instant::now() + Duration::from_secs(1);
                }
            }
        }

        // Detect .ipa files copied in by the "+" tile flow and refresh the
        // grid once the new file has finished copying (its size stops
        // changing and has stayed stable for a moment).
        if let Some(watch) = &mut awaited_ipa {
            let new_listing = list_top_level_ipa_files(&apps_dir);
            if new_listing != watch.last_seen {
                watch.last_seen = new_listing;
                watch.dirty = true;
                watch.last_change = Some(Instant::now());
            } else if watch.dirty
                && watch
                    .last_change
                    .is_some_and(|t| t.elapsed() >= IPA_COPY_SETTLE_TIME)
            {
                watch.dirty = false;
                if let Some(grid) = icon_grid_stuff.as_mut() {
                    let page_count =
                        reload_app_grid(env, grid, &mut apps, &mut current_page, &apps_dir);
                    rebuild_page_dots(env, main_view, app_frame.size, &mut page_dots, page_count);
                    update_page_dots(env, &page_dots, current_page);
                }
            }
        }
    };

    // Apply user-specified overrides
    if let Some(scale_hack) = picker_settings.scale_hack {
        option_args.push(format!("--scale-hack={}", scale_hack.get()));
    }
    if let Some(orientation) = picker_settings.orientation {
        option_args.push(
            match orientation {
                DeviceOrientation::LandscapeLeft => "--landscape-left",
                DeviceOrientation::LandscapeRight => "--landscape-right",
                DeviceOrientation::PortraitUpsideDown => "--upside-down",
                _ => todo!(),
            }
            .to_string(),
        );
    }
    if picker_settings.fullscreen {
        option_args.push("--fullscreen".to_string());
    } else {
        option_args.push("--no-fullscreen".to_string());
    }
    if show_fullscreen_row {
        option_args.push(
            if picker_settings.preserve_aspect_ratio {
                "--preserve-aspect-ratio"
            } else {
                "--no-preserve-aspect-ratio"
            }
            .to_string(),
        );
    }
    if !picker_settings.analog_stick_tilt_controls {
        option_args.push("--disable-analog-stick-tilt-controls".to_string());
    } else {
        option_args.push("--enable-analog-stick-tilt-controls".to_string());
    }
    if picker_settings.network {
        option_args.push("--allow-network-access".to_string());
    } else {
        option_args.push("--no-network-access".to_string());
    }
    option_args.push(quick_options_trainer_argument(picker_settings.cheat_engine).to_string());
    // Saved diagnostic switches override command-line and options-file values.
    option_args
        .push(quick_options_trace_gl_errors_argument(picker_settings.trace_gl_errors).to_string());
    option_args
        .push(quick_options_trace_gl_draws_argument(picker_settings.trace_gl_draws).to_string());
    option_args.push(
        if picker_settings.game_center_authenticated {
            "--game-center-authenticated"
        } else {
            "--no-game-center-authenticated"
        }
        .to_string(),
    );

    env.window_mut().set_performance_overlay_settings(
        quick_options_show_fps,
        picker_settings.overlay_mode,
        picker_settings.overlay_corner,
    );
    option_args.push(format!(
        "--overlay={}",
        if quick_options_show_fps { "on" } else { "off" }
    ));
    option_args.push(format!(
        "--overlay-mode={}",
        picker_settings.overlay_mode.short_name()
    ));
    option_args.push(format!(
        "--overlay-corner={}",
        picker_settings.overlay_corner.short_name()
    ));

    option_args.push(
        quick_options_force_composition_argument(picker_settings.force_composition).to_string(),
    );

    // Always passed explicitly so the picker's renderer choice wins over
    // options-file settings, including legacy --gles-native switches.
    option_args
        .push(quick_options_renderer_mode_argument(picker_settings.renderer_mode).to_string());
    // The two compatibility fixes are only emitted when the Settings screen
    // actually set them (see `quick_options_fix_arguments`).
    option_args.extend(quick_options_fix_arguments(
        picker_settings.rotation_fix,
        picker_settings.fix_movie_player,
    ));

    if let Some(tag) = device_state.selected {
        let tag = tag as NSInteger;
        if tag == DEVICE_TAG_DEFAULT {
            // No override — fall back to the app bundle / built-in default.
        } else if tag == DEVICE_TAG_AUTO {
            option_args.push("--device-family=auto".to_string());
        } else if let Some(family) = crate::window::DeviceFamily::for_tag(tag as usize) {
            option_args.push(format!("--device-family={}", family.option_name()));
        }
    }

    if let Some(arg) = language_argument(language_state.selected) {
        option_args.push(arg);
    }

    option_args.extend(quick_options_picker_settings_arguments(&picker_settings));
    option_args.extend(picker_settings.gles2_mode.arguments());

    // Return the environment so some parts of it can be salvaged.
    (app_path, option_args)
}

/// Old-iOS home screen layout: a fixed 4×4 icon grid above a row of page dots,
/// and a translucent dock along the bottom edge.
const APP_PICKER_ICON_COLS: usize = 4;
const APP_PICKER_ICON_ROWS: usize = 4;
const APP_PICKER_GRID_TOP: CGFloat = 6.0;
const APP_PICKER_SIDE_MARGIN: CGFloat = 10.0;
const APP_PICKER_ICON_MAX_SIZE: CGFloat = 60.0;
const APP_PICKER_ICON_MIN_SIZE: CGFloat = 24.0;
const APP_PICKER_ICON_GAP_X_MIN: CGFloat = 10.0;
const APP_PICKER_ICON_GAP_X_MAX: CGFloat = 20.0;
const APP_PICKER_LABEL_HEIGHT: CGFloat = 14.0;
const APP_PICKER_LABEL_TOP_GAP: CGFloat = 2.0;
const APP_PICKER_ROW_GAP_MIN: CGFloat = 2.0;
const APP_PICKER_DOTS_ROW_HEIGHT: CGFloat = 18.0;
const APP_PICKER_DOCK_HEIGHT: CGFloat = 88.0;

const PAGE_DOT_SIZE: CGFloat = 7.0;
const PAGE_DOT_GAP: CGFloat = 9.0;

/// Side length of the bitmap used for app and dock icons. The on-screen size
/// is chosen separately by [app_picker_layout] so that the grid always fits.
const ICON_SIZE: CGSize = CGSize {
    width: 72.0,
    height: 72.0,
};

/// Vertical centre-line of the page-dot row, just above the dock.
fn app_picker_dots_top(app_height: CGFloat) -> CGFloat {
    app_picker_grid_bottom(app_height) + (APP_PICKER_DOTS_ROW_HEIGHT - PAGE_DOT_SIZE) / 2.0
}

/// The bottom edge of the icon grid: the page-dot/version row sits below it.
fn app_picker_grid_bottom(app_height: CGFloat) -> CGFloat {
    app_height - APP_PICKER_DOCK_HEIGHT - APP_PICKER_DOTS_ROW_HEIGHT
}

/// Geometry of the 4×4 icon grid.
#[derive(Clone, Copy, Debug)]
struct AppPickerLayout {
    /// Side length of one (square) icon cell.
    icon_size: CGFloat,
    /// Horizontal distance between icon cells.
    icon_gap_x: CGFloat,
    /// Vertical distance between icon rows.
    icon_gap_y: CGFloat,
    /// Top-left corner of the first icon cell.
    grid_origin: CGPoint,
    /// Size of an icon label, also used to space the rows.
    label_size: CGSize,
}

fn app_picker_layout(screen_width: CGFloat, screen_height: CGFloat) -> AppPickerLayout {
    let cols = APP_PICKER_ICON_COLS as CGFloat;
    let rows = APP_PICKER_ICON_ROWS as CGFloat;
    let inner_width = (screen_width - APP_PICKER_SIDE_MARGIN * 2.0).max(1.0);
    let row_chrome = APP_PICKER_LABEL_TOP_GAP + APP_PICKER_LABEL_HEIGHT;
    let available_height = (app_picker_grid_bottom(screen_height) - APP_PICKER_GRID_TOP).max(1.0);

    // The icons must fit both across (four columns) and down (four rows plus
    // labels). Small screens get smaller icons rather than fewer cells.
    let icon_by_width = (inner_width - APP_PICKER_ICON_GAP_X_MIN * (cols - 1.0)) / cols;
    let icon_by_height =
        (available_height - rows * row_chrome - APP_PICKER_ROW_GAP_MIN * (rows - 1.0)) / rows;
    let icon_size = icon_by_width
        .min(icon_by_height)
        .clamp(APP_PICKER_ICON_MIN_SIZE, APP_PICKER_ICON_MAX_SIZE)
        .floor();
    let icon_gap_x =
        ((inner_width - icon_size * cols) / (cols - 1.0)).clamp(0.0, APP_PICKER_ICON_GAP_X_MAX);
    let grid_width = icon_size * cols + icon_gap_x * (cols - 1.0);
    let cell_height = icon_size + row_chrome;
    let icon_gap_y =
        ((available_height - rows * cell_height) / (rows - 1.0)).max(APP_PICKER_ROW_GAP_MIN);

    AppPickerLayout {
        icon_size,
        icon_gap_x,
        icon_gap_y,
        grid_origin: CGPoint {
            x: (screen_width - grid_width) / 2.0,
            y: APP_PICKER_GRID_TOP,
        },
        label_size: CGSize {
            width: icon_size + icon_gap_x,
            height: APP_PICKER_LABEL_HEIGHT,
        },
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn classic_phone_picker_has_four_by_four_grid() {
        // A visible status bar leaves `UIScreen.applicationFrame` at 320x460.
        let layout = app_picker_layout(320.0, 460.0);
        assert_eq!(APP_PICKER_ICON_COLS, 4);
        assert_eq!(APP_PICKER_ICON_ROWS, 4);
        assert_eq!(layout.icon_size, 60.0);

        let last_row_bottom = layout.grid_origin.y
            + 4.0 * layout.icon_size
            + 3.0 * layout.icon_gap_y
            + 4.0 * (APP_PICKER_LABEL_TOP_GAP + APP_PICKER_LABEL_HEIGHT);
        assert!(last_row_bottom <= app_picker_grid_bottom(460.0) + 0.5);
    }

    #[test]
    fn pages_hold_one_gridful_of_apps() {
        assert_eq!(compute_pages(16, 0), vec![0..0]);
        assert_eq!(compute_pages(16, 16), vec![0..16]);
        assert_eq!(compute_pages(16, 17), vec![0..16, 16..17]);
    }
}

/// One full set of icon-grid cells on its own container view. Two of these
/// exist so that one page can slide out while the next slides in.
struct PageContainer {
    view: id,
    /// (icon button, name label) pairs.
    cells: Vec<(id, id)>,
}

struct IconGridStuff {
    containers: Vec<PageContainer>,
    /// Index into `containers` of the container currently on screen.
    visible: usize,
    /// Width of the picker screen, used by the page slide animation.
    screen_width: CGFloat,
    placeholder_icon: Option<id>,
    pages: Vec<std::ops::Range<usize>>,
    /// Maps an on-screen icon cell's button to the app it shows.
    icon_map: HashMap<id, usize>,
}

fn make_icon_grid(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    app_frame: CGRect,
    total_app_count: usize,
) -> IconGridStuff {
    let layout = app_picker_layout(app_frame.size.width, app_frame.size.height);
    let num_cols = APP_PICKER_ICON_COLS;
    let num_rows = APP_PICKER_ICON_ROWS;
    let cell_step_x = layout.icon_size + layout.icon_gap_x;
    let cell_step_y =
        layout.icon_size + APP_PICKER_LABEL_TOP_GAP + layout.label_size.height + layout.icon_gap_y;

    // The icon image view is inset slightly so the rounded corners aren't
    // clipped.
    let icon_inset = (layout.icon_size * 0.03).max(0.0);

    let icon_tapped_sel = env.objc.lookup_selector("iconTapped:").unwrap();
    let icon_touch_down_sel = env.objc.lookup_selector("iconTouchDown:").unwrap();
    let icon_touch_cancelled_sel = env.objc.lookup_selector("iconTouchCancelled:").unwrap();

    let mut containers = Vec::new();
    for container_idx in 0..2 {
        // The container is a full-screen view, so moving it slides all its cells.
        let clear: id = msg_class![env; UIColor clearColor];
        let container = new_view(
            env,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: app_frame.size,
            },
            clear,
        );
        if container_idx != 0 {
            () = msg![env; container setHidden:true];
        }
        () = msg![env; main_view addSubview:container];

        let mut cells = Vec::new();
        for i in 0..(num_cols * num_rows) {
            let col = i % num_cols;
            let row = i / num_cols;

            // Rounding is needed here to avoid a blurry or offset image.
            let icon_frame = CGRect {
                origin: CGPoint {
                    x: (layout.grid_origin.x + (col as CGFloat) * cell_step_x).round(),
                    y: (layout.grid_origin.y + (row as CGFloat) * cell_step_y).round(),
                },
                size: CGSize {
                    width: layout.icon_size,
                    height: layout.icon_size,
                },
            };
            let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
            () = msg![env; icon_button setFrame:icon_frame];
            let image_view: id = msg![env; icon_button imageView];
            let bounds: CGRect = msg![env; icon_button bounds];
            () = msg![env; image_view setFrame:(CGRect {
                origin: CGPoint {
                    x: icon_inset,
                    y: icon_inset,
                },
                size: CGSize {
                    width: (bounds.size.width - icon_inset * 2.0).max(1.0),
                    height: (bounds.size.height - icon_inset * 2.0).max(1.0),
                },
            })];
            let layer: id = msg![env; image_view layer];
            let gravity = ns_string::get_static_str(env, "resizeAspect");
            () = msg![env; layer setContentsGravity:gravity];
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_tapped_sel
                                forControlEvents:UIControlEventTouchUpInside];
            // Finger-down starts a possible long press; leaving the icon or
            // lifting the finger outside it cancels that.
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_touch_down_sel
                                forControlEvents:UIControlEventTouchDown];
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_touch_cancelled_sel
                                forControlEvents:(UIControlEventTouchDragExit
                                    | UIControlEventTouchUpOutside
                                    | UIControlEventTouchCancel)];
            () = msg![env; container addSubview:icon_button];

            // Rounding is needed here to avoid blurry text.
            let label_frame = CGRect {
                origin: CGPoint {
                    x: (icon_frame.origin.x - (layout.label_size.width - layout.icon_size) / 2.0)
                        .round(),
                    y: (icon_frame.origin.y + layout.icon_size + APP_PICKER_LABEL_TOP_GAP).round(),
                },
                size: layout.label_size,
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            let font_size: CGFloat = layout.label_size.height - 3.0;
            let font: id = msg_class![env; UIFont boldSystemFontOfSize:font_size];
            () = msg![env; label setFont:font];
            // White with a soft shadow, like the old iOS home screen.
            let text_color: id = msg_class![env; UIColor whiteColor];
            () = msg![env; label setTextColor:text_color];
            let shadow_color = ui_color(env, 0.0, 0.0, 0.0, 0.7);
            () = msg![env; label setShadowColor:shadow_color];
            () = msg![env; label setShadowOffset:(CGSize {
                width: 0.0,
                height: 1.0,
            })];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; container addSubview:label];

            cells.push((icon_button, label));
        }
        containers.push(PageContainer {
            view: container,
            cells,
        });
    }

    let total_slots = containers[0].cells.len();
    let pages = compute_pages(total_slots, total_app_count);

    IconGridStuff {
        containers,
        visible: 0,
        screen_width: app_frame.size.width,
        placeholder_icon: None,
        pages,
        icon_map: HashMap::new(),
    }
}

/// Work out which apps go on each page of the icon grid. Every page is a full
/// 4×4 grid; the "+" tile and Settings live in the dock instead of taking
/// grid slots.
fn compute_pages(total_slots: usize, total_app_count: usize) -> Vec<std::ops::Range<usize>> {
    let total_slots = total_slots.max(1);
    let page_count = ((total_app_count + total_slots - 1) / total_slots).max(1);
    (0..page_count)
        .map(|page_idx| {
            let start = page_idx * total_slots;
            start..(start + total_slots).min(total_app_count)
        })
        .collect()
}

/// Renders a rounded, glossed app icon with the given background colour.
/// `draw` paints on top of the background at bitmap resolution, in
/// top-left-origin coordinates (y grows downwards).
fn make_icon(
    env: &mut Environment,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    draw: impl FnOnce(&mut Environment, id),
) -> id {
    let ui_scale = env.options.ui_scale.get() as CGFloat;
    let color_space = CGColorSpaceCreateDeviceRGB(env);
    let context = CGBitmapContextCreate(
        env,
        Ptr::null(),
        (ICON_SIZE.width as u32).saturating_mul(env.options.ui_scale.get()),
        (ICON_SIZE.height as u32).saturating_mul(env.options.ui_scale.get()),
        8,
        4 * (ICON_SIZE.width as u32).saturating_mul(env.options.ui_scale.get()),
        color_space,
        kCGImageAlphaPremultipliedLast,
    );
    UIGraphicsPushContext(env, context);

    let scaled_width = ICON_SIZE.width * ui_scale;
    let scaled_height = ICON_SIZE.height * ui_scale;

    // The bitmap's rows are stored bottom-up; flip the transform so the
    // background and `draw` can use screen-like top-down coordinates.
    CGContextTranslateCTM(env, context, 0.0, scaled_height);
    CGContextScaleCTM(env, context, 1.0, -1.0);

    let (r, g, b, a) = bg_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    CGContextFillRect(
        env,
        context,
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: scaled_width,
                height: scaled_height,
            },
        },
    );

    draw(env, context);

    UIGraphicsPopContext(env);

    let cg_image = CGBitmapContextCreateImage(env, context);
    // Same gloss and (resolution-scaled) corner radius as `Bundle::load_icon()`.
    cg_image::borrow_image_mut(&mut env.objc, cg_image)
        .round_icon_corners(/* add_sheen: */ true);
    CGContextRelease(env, context);

    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);

    ui_image
}

/// Builds an icon with white text in the middle, e.g. "+" or "WWW".
fn make_icon_from_text(
    env: &mut Environment,
    text: &str,
    font_size: CGFloat,
    baseline_offset: CGFloat,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
) -> id {
    make_icon(env, bg_color, |env, context| {
        let ui_scale = env.options.ui_scale.get() as CGFloat;
        let scaled_width = ICON_SIZE.width * ui_scale;
        let scaled_height = ICON_SIZE.height * ui_scale;
        let font: id = msg_class![env; UIFont systemFontOfSize:(font_size * ui_scale)];
        let string: id = ns_string::from_rust_string(env, text.to_string());
        let size: CGSize = msg![env; string sizeWithFont:font];
        CGContextSetRGBFillColor(env, context, 1.0, 1.0, 1.0, 1.0); // white
        let origin = CGPoint {
            x: scaled_width / 2.0 - size.width / 2.0,
            y: scaled_height / 2.0 - size.height / 2.0 + baseline_offset * ui_scale,
        };
        let _: CGSize = msg![env; string drawAtPoint:origin withFont:font];
        release(env, string);
    })
}

/// Fills one page container (`container_idx`) of the icon grid with the apps
/// of `page_idx`. The other container is left alone, so it can keep showing
/// the previous page while a slide animation runs.
fn update_icon_grid(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    apps: &mut [AppInfo],
    page_idx: usize,
    container_idx: usize,
) {
    let cells = icon_grid_stuff.containers[container_idx].cells.clone();
    for &(icon_button, _) in &cells {
        icon_grid_stuff.icon_map.remove(&icon_button);
    }

    let app_idx_range = icon_grid_stuff.pages[page_idx].clone();
    let mut icon_iter = cells.iter();

    for app_idx in app_idx_range {
        let app = &mut apps[app_idx];

        let &(icon_button, label) = icon_iter.next().unwrap();

        if let Some(icon) = app.icon.take() {
            let image = cg_image::from_image(env, icon);
            let image: id = msg_class![env; UIImage imageWithCGImage:image];
            app.icon_ui_image = Some(image);
        }

        let image = app.icon_ui_image.unwrap_or_else(|| {
            *icon_grid_stuff.placeholder_icon.get_or_insert_with(|| {
                make_icon_from_text(env, "?", 40.0, 0.0, (0.5, 0.5, 0.5, 1.0))
            })
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];

        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];

        icon_grid_stuff.icon_map.insert(icon_button, app_idx);
    }

    // Blank the unused cells at the end of the last page.
    for &(icon_button, label) in icon_iter {
        () = msg![env; icon_button setImage:nil forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
    }
}

/// Construct and display the iPhone OS 4 lock screen over the picker wallpaper.
/// A rightward swipe dismisses the lock screen; a pulsing label recreates the
/// animated “slide to unlock” treatment.
fn run_lock_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    parent: id,
    size: CGSize,
    grid: Option<&IconGridStuff>,
    status_bar: Option<id>,
    status_time_label: Option<id>,
    dock: id,
    version_label: id,
    page_dots: &[id],
) {
    if let Some(grid) = grid {
        for container in &grid.containers {
            let container_view = container.view;
            () = msg![env; container_view setHidden:true];
        }
    }
    () = msg![env; dock setHidden:true];
    () = msg![env; version_label setHidden:true];
    for &dot in page_dots {
        () = msg![env; dot setHidden:true];
    }
    let clear: id = msg_class![env; UIColor clearColor];
    let lock = new_view(env, rect(0.0, 0.0, size.width, size.height), clear);
    if let Some(bar) = status_bar {
        let status_glass = ui_color(env, 0.02, 0.025, 0.04, 0.36);
        () = msg![env; bar setBackgroundColor:status_glass];
    }

    // Layered translucent bands give the clock panel the soft glass gradient
    // of the iOS 4 lock screen while keeping the wallpaper visible beneath it.
    let top = new_view(env, rect(0.0, 0.0, size.width, 138.0), clear);
    const GLASS_STEPS: usize = 48;
    for band in 0..GLASS_STEPS {
        let t = band as CGFloat / (GLASS_STEPS - 1) as CGFloat;
        let y = 138.0 * t;
        let band_height = 138.0 / GLASS_STEPS as CGFloat + 0.5;
        let tint = ui_color(env, 0.025, 0.035, 0.06, 0.30 - 0.18 * t);
        let strip = new_view(env, rect(0.0, y, size.width, band_height), tint);
        () = msg![env; top addSubview:strip];
    }
    () = msg![env; lock addSubview:top];
    let header_edge_color = ui_color(env, 1.0, 1.0, 1.0, 0.18);
    let header_edge = new_view(env, rect(0.0, 137.0, size.width, 1.0), header_edge_color);
    () = msg![env; lock addSubview:header_edge];
    let white: id = msg_class![env; UIColor whiteColor];
    let clock = new_label(
        env,
        rect(0.0, 12.0, size.width, 82.0),
        &lock_screen_time_string(),
        68.0,
        false,
        white,
    );
    () = msg![env; clock setTextAlignment:UITextAlignmentCenter];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);
    () = msg![env; clock setShadowColor:shadow];
    () = msg![env; clock setShadowOffset:(CGSize { width: 0.0, height: 2.0 })];
    () = msg![env; lock addSubview:clock];
    let date = new_label(
        env,
        rect(8.0, 91.0, size.width - 16.0, 32.0),
        &lock_screen_date_string(),
        22.0,
        false,
        white,
    );
    () = msg![env; date setTextAlignment:UITextAlignmentCenter];
    () = msg![env; date setShadowColor:shadow];
    () = msg![env; date setShadowOffset:(CGSize { width: 0.0, height: 1.0 })];
    () = msg![env; lock addSubview:date];

    let bottom = size.height - 116.0;
    let shade = new_view(env, rect(0.0, bottom - 24.0, size.width, 140.0), clear);
    for band in 0..GLASS_STEPS {
        let t = band as CGFloat / (GLASS_STEPS - 1) as CGFloat;
        let y = 140.0 * t;
        let band_height = 140.0 / GLASS_STEPS as CGFloat + 0.5;
        let tint = ui_color(env, 0.015, 0.02, 0.035, 0.035 + 0.20 * t);
        let strip = new_view(env, rect(0.0, y, size.width, band_height), tint);
        () = msg![env; shade addSubview:strip];
    }
    () = msg![env; lock addSubview:shade];
    // Raised metal bezel around the black glass slider, like the inset 3D
    // capsule on the reference iPhone OS lock screen.
    let bezel_color = ui_color(env, 0.25, 0.26, 0.28, 1.0);
    let bezel = new_view(
        env,
        rect(17.0, bottom + 10.0, size.width - 34.0, 68.0),
        bezel_color,
    );
    let bezel_layer: id = msg![env; bezel layer];
    () = msg![env; bezel_layer setCornerRadius:(20.0 as CGFloat)];
    let bezel_edge = ui_color(env, 0.62, 0.64, 0.67, 1.0);
    let bezel_edge_cg: id = msg![env; bezel_edge CGColor];
    () = msg![env; bezel_layer setBorderColor:bezel_edge_cg];
    () = msg![env; bezel_layer setBorderWidth:(1.5 as CGFloat)];
    () = msg![env; lock addSubview:bezel];

    let track_color = ui_color(env, 0.008, 0.012, 0.02, 0.90);
    let track = new_view(
        env,
        rect(21.0, bottom + 13.0, size.width - 42.0, 62.0),
        track_color,
    );
    let track_layer: id = msg![env; track layer];
    () = msg![env; track_layer setCornerRadius:(18.0 as CGFloat)];
    () = msg![env; track setClipsToBounds:true];
    let track_edge = ui_color(env, 0.0, 0.0, 0.0, 0.85);
    let track_edge_cg: id = msg![env; track_edge CGColor];
    () = msg![env; track_layer setBorderColor:track_edge_cg];
    () = msg![env; track_layer setBorderWidth:(1.0 as CGFloat)];
    () = msg![env; lock addSubview:track];
    // A single restrained top glint keeps the capsule glossy without visible
    // striping on renderers that do not composite translucent bands smoothly.
    let track_glint_color = ui_color(env, 1.0, 1.0, 1.0, 0.11);
    let track_glint = new_view(
        env,
        rect(14.0, 2.0, size.width - 70.0, 1.0),
        track_glint_color,
    );
    () = msg![env; track addSubview:track_glint];
    let track_width = size.width - 42.0;
    let thumb_width = (track_width * 0.245).clamp(76.0, 100.0);
    let label_x = thumb_width + 24.0;
    let label_width = (track_width - label_x - 12.0).max(0.0);
    // Render one label per glyph. The custom renderer does not reliably clip
    // duplicate full-string labels, so animating individual glyph alpha gives
    // a clean left-to-right sheen without any overprinted text.
    let shimmer_text = "slide to unlock";
    let font: id = msg_class![env; UIFont systemFontOfSize:(25.0 as CGFloat)];
    let whole_text = ns_string::from_rust_string(env, shimmer_text.to_string());
    let whole_size: CGSize = msg![env; whole_text sizeWithFont:font];
    release(env, whole_text);
    let mut shimmer_letters = Vec::new();
    let mut prefix = String::new();
    let mut prefix_width = 0.0 as CGFloat;
    let text_start = label_x + (label_width - whole_size.width) * 0.5;
    for glyph in shimmer_text.chars() {
        prefix.push(glyph);
        let prefix_string = ns_string::from_rust_string(env, prefix.clone());
        let size: CGSize = msg![env; prefix_string sizeWithFont:font];
        release(env, prefix_string);
        let glyph_width = (size.width - prefix_width).max(1.0);
        let glyph_text = glyph.to_string();
        let color = ui_color(env, 1.0, 1.0, 1.0, 0.86);
        let letter = new_label(
            env,
            rect(text_start + prefix_width, 0.0, glyph_width + 0.5, 62.0),
            &glyph_text,
            25.0,
            false,
            color,
        );
        () = msg![env; letter setTextAlignment:UITextAlignmentCenter];
        () = msg![env; track addSubview:letter];
        shimmer_letters.push(letter);
        prefix_width = size.width;
    }
    // The classic iOS thumb is a wide silver pill, not a square tile.
    // Keep its proportion tied to the track so it scales with device width.
    let thumb_height = 54.0;
    let thumb_image = make_lock_thumb_image(env);
    let knob: id = msg_class![env; UIImageView alloc];
    let knob: id = msg![env; knob initWithImage:thumb_image];
    () = msg![env; knob setFrame:(rect(4.0, 4.0, thumb_width, thumb_height))];
    () = msg![env; track addSubview:knob];
    let arrow_image = make_lock_arrow_image(env);
    let arrow_view: id = msg_class![env; UIImageView alloc];
    let arrow_view: id = msg![env; arrow_view initWithImage:arrow_image];
    let arrow_width = 52.0;
    let arrow_height = 40.0;
    () = msg![env; arrow_view setFrame:(rect(
        (thumb_width - arrow_width) * 0.5,
        (thumb_height - arrow_height) * 0.5,
        arrow_width,
        arrow_height,
    ))];
    () = msg![env; knob addSubview:arrow_view];

    // A nearly transparent UISlider is the actual touch surface. Its value
    // drives the visible thumb continuously, so the arrow follows the finger
    // instead of only reacting after a swipe has already ended.
    let slider: id = msg_class![env; UISlider alloc];
    // Align UISlider's touch-value endpoints with the centers of the wide
    // custom thumb so the visual knob stays directly under the finger.
    let slider_start = 4.0 + thumb_width * 0.5 - 11.5;
    let slider_width = track_width - slider_start * 2.0;
    let slider: id = msg![env; slider initWithFrame:(rect(
        slider_start,
        0.0,
        slider_width,
        62.0,
    ))];
    () = msg![env; slider setValue:0.0f32];
    // Keep the control itself fully hit-testable, but hide its stock blue/gray
    // track and thumb so only our custom glass slider is visible.
    let slider_parts: id = msg![env; slider subviews];
    let slider_part_count: usize = msg![env; slider_parts count];
    for index in 0..slider_part_count {
        let part: id = msg![env; slider_parts objectAtIndex:(index as usize)];
        () = msg![env; part setAlpha:(0.0 as CGFloat)];
    }
    let slider_selector = env.objc.lookup_selector("lockSliderChanged:").unwrap();
    () = msg![env; slider addTarget:delegate
                              action:slider_selector
                    forControlEvents:UIControlEventValueChanged];
    () = msg![env; track addSubview:slider];
    () = msg![env; parent addSubview:lock];

    {
        let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host.lock_screen_unlocked = false;
        host.lock_slider_progress = 0.0;
    }
    let started = Instant::now();
    let mut clock_updated = Instant::now();
    let mut shimmer_updated = Instant::now() - Duration::from_millis(34);
    let mut last_knob_progress = -1.0f32;
    loop {
        run_run_loop_single_iteration(env, run_loop);
        let (unlocked, mut progress) = {
            let host = env.objc.borrow::<AppPickerDelegateHostObject>(delegate);
            (host.lock_screen_unlocked, host.lock_slider_progress)
        };
        if unlocked {
            break;
        }
        let tracking: bool = msg![env; slider tracking];
        if !tracking && progress > 0.0 {
            // A partial swipe eases back to the start; a completed one is
            // handled above by the unlock transition.
            progress = (progress - 0.055).max(0.0);
            () = msg![env; slider setValue:progress];
            env.objc
                .borrow_mut::<AppPickerDelegateHostObject>(delegate)
                .lock_slider_progress = progress;
        }
        if shimmer_updated.elapsed() >= Duration::from_millis(33) {
            shimmer_updated = Instant::now();
            let elapsed = started.elapsed().as_secs_f64();
            let sweep = (elapsed % 2.4) / 2.4 * (shimmer_letters.len() as f64 + 4.0) - 2.0;
            for (index, &letter) in shimmer_letters.iter().enumerate() {
                let distance = (index as f64 - sweep).abs();
                let glow = (1.0 - distance / 2.2).clamp(0.0, 1.0) as CGFloat;
                () = msg![env; letter setAlpha:(0.58 + glow * 0.42)];
            }
        }
        if progress != last_knob_progress {
            last_knob_progress = progress;
            let mut knob_frame: CGRect = msg![env; knob frame];
            let travel = (track_width - thumb_width - 8.0).max(0.0);
            knob_frame.origin.x = 4.0 + travel * progress as CGFloat;
            () = msg![env; knob setFrame:knob_frame];
        }
        if clock_updated.elapsed() >= Duration::from_secs(1) {
            clock_updated = Instant::now();
            // Keep both clocks on the same timestamp while the lock screen is
            // modal; the outer picker event loop is paused until unlock.
            let text = ns_string::from_rust_string(env, lock_screen_time_string());
            () = msg![env; clock setText:text];
            if let Some(status_time_label) = status_time_label {
                () = msg![env; status_time_label setText:text];
            }
            release(env, text);
        }
    }
    {
        let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host.lock_screen_unlocked = false;
        host.lock_slider_progress = 0.0;
        host.swipe_right = false;
        host.swipe_left = false;
    }
    animate_for(
        env,
        run_loop,
        delegate,
        Duration::from_millis(480),
        |env, t| {
            let p = ease_in_out_cubic(t) as CGFloat;
            let mut frame: CGRect = msg![env; lock frame];
            frame.origin.y = -size.height * p;
            () = msg![env; lock setFrame:frame];
        },
    );
    () = msg![env; lock setHidden:true];
    if let Some(bar) = status_bar {
        () = msg![env; bar setBackgroundColor:clear];
    }
    if let Some(grid) = grid {
        let active_container = grid.containers[grid.visible].view;
        () = msg![env; active_container setHidden:false];
    }
    () = msg![env; dock setHidden:false];
    () = msg![env; version_label setHidden:false];
    for &dot in page_dots {
        () = msg![env; dot setHidden:false];
    }
}

fn make_lock_thumb_image(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/lock_thumb.png"));
    let image = Image::from_bytes(bytes).unwrap();
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

fn make_lock_arrow_image(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/lock_arrow.png"));
    let image = Image::from_bytes(bytes).unwrap();
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

#[cfg(unix)]
fn lock_screen_calendar_parts() -> (i32, i32, i32, i32, i32, i32) {
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return (14, 50, 3, 0, 21, 2026);
        }
        (
            tm.tm_hour,
            tm.tm_min,
            tm.tm_wday,
            tm.tm_mon,
            tm.tm_mday,
            tm.tm_year + 1900,
        )
    }
}
#[cfg(not(unix))]
fn lock_screen_calendar_parts() -> (i32, i32, i32, i32, i32, i32) {
    (14, 50, 3, 0, 21, 2026)
}
fn lock_screen_time_string() -> String {
    let (hour, minute, ..) = lock_screen_calendar_parts();
    format!("{:02}:{:02}", hour, minute)
}
fn lock_screen_date_string() -> String {
    let (_, _, weekday, month, day, _) = lock_screen_calendar_parts();
    let weekdays = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    format!(
        "{}, {} {}",
        weekdays[weekday.clamp(0, 6) as usize],
        months[month.clamp(0, 11) as usize],
        day
    )
}

/// Builds a fake old-iOS status bar: signal bars, carrier, clock and battery.
/// Returns the bar itself, the clock label and the battery charge bar, which
/// the event loop updates.
fn make_status_bar(
    env: &mut Environment,
    delegate: id,
    width: CGFloat,
    height: CGFloat,
) -> (id, id, id) {
    let clear: id = msg_class![env; UIColor clearColor];
    let bar = new_view(env, rect(0.0, 0.0, width, height), clear);
    let white: id = msg_class![env; UIColor whiteColor];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);

    // Signal strength: four white bars of increasing height.
    let signal_width: CGFloat = 3.0;
    let signal_gap: CGFloat = 1.5;
    let signal_bottom = height - 4.0;
    for (idx, signal_height) in [4.0 as CGFloat, 6.0, 8.0, 10.0].iter().enumerate() {
        let signal = new_view(
            env,
            rect(
                5.0 + idx as CGFloat * (signal_width + signal_gap),
                signal_bottom - *signal_height,
                signal_width,
                *signal_height,
            ),
            white,
        );
        () = msg![env; bar addSubview:signal];
    }

    let carrier = new_label(
        env,
        rect(27.0, 1.0, width * 0.35, height - 2.0),
        "HyperHLE",
        12.0,
        true,
        white,
    );
    () = msg![env; carrier setShadowColor:shadow];
    () = msg![env; carrier setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; bar addSubview:carrier];

    let time = new_label(
        env,
        rect(0.0, 1.0, width, height - 2.0),
        &status_bar_time_string(),
        12.0,
        true,
        white,
    );
    () = msg![env; time setTextAlignment:UITextAlignmentCenter];
    () = msg![env; time setShadowColor:shadow];
    () = msg![env; time setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; bar addSubview:time];

    // Battery: a white rounded shell with a black inside and a charge bar.
    let battery = new_view(
        env,
        rect(width - BATTERY_RIGHT_INSET, 5.0, 24.0, 11.0),
        white,
    );
    let battery_layer: id = msg![env; battery layer];
    () = msg![env; battery_layer setCornerRadius:(2.5 as CGFloat)];
    () = msg![env; bar addSubview:battery];

    let black: id = msg_class![env; UIColor blackColor];
    let battery_inner = new_view(
        env,
        rect(width - BATTERY_RIGHT_INSET + 1.5, 6.5, 21.0, 8.0),
        black,
    );
    () = msg![env; bar addSubview:battery_inner];
    let charge = new_view(
        env,
        rect(
            width - BATTERY_CHARGE_LEFT,
            7.5,
            BATTERY_CHARGE_MAX_WIDTH,
            6.0,
        ),
        white,
    );
    () = msg![env; bar addSubview:charge];
    let nub = new_view(
        env,
        rect(width - BATTERY_RIGHT_INSET + 24.5, 8.5, 2.0, 4.0),
        white,
    );
    () = msg![env; bar addSubview:nub];
    access_prompt::add_status_target(env, delegate, bar, width);

    (bar, time, charge)
}

/// Left edge of the battery shell, measured from the right of the screen.
const BATTERY_RIGHT_INSET: CGFloat = 31.0;
/// Left edge of the charge bar, measured from the right of the screen.
const BATTERY_CHARGE_LEFT: CGFloat = 28.5;
/// Width of the charge bar when the battery is full.
const BATTERY_CHARGE_MAX_WIDTH: CGFloat = 15.0;

/// Updates the status bar's battery charge bar for the given percentage and
/// charging state, like the old iOS status bar: the fill grows with the level
/// and turns red when the battery is low.
fn update_battery_indicator(
    env: &mut Environment,
    charge_view: id,
    level: Option<(i32, crate::window::BatteryState)>,
) {
    let (percent, charging) = match level {
        Some((percent, state)) if percent >= 0 => {
            let charging = matches!(
                state,
                crate::window::BatteryState::Charging | crate::window::BatteryState::Full
            );
            (percent.clamp(0, 100) as CGFloat, charging)
        }
        // No battery information (a desktop, or SDL couldn't read the level):
        // show a full battery, as the picker always did before the level was
        // wired up. SDL reports -1 when the level is unknown.
        _ => (100.0, false),
    };
    let full: id = msg_class![env; UIColor whiteColor];
    let color: id = if charging {
        ui_color(env, 0.20, 0.85, 0.30, 1.0)
    } else if percent <= 20.0 {
        ui_color(env, 0.90, 0.15, 0.15, 1.0)
    } else {
        full
    };
    () = msg![env; charge_view setBackgroundColor:color];
    let width = if percent <= 0.0 {
        0.0
    } else {
        (BATTERY_CHARGE_MAX_WIDTH * percent / 100.0).max(1.5)
    };
    let frame: CGRect = msg![env; charge_view frame];
    () = msg![env; charge_view setFrame:(rect(
        frame.origin.x,
        7.5,
        width,
        6.0
    ))];
}

#[cfg(unix)]
fn status_bar_time_string() -> String {
    // SAFETY: `localtime_r` only writes into the provided `tm`.
    let tm = unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "9:41".to_string();
        }
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

#[cfg(not(unix))]
fn status_bar_time_string() -> String {
    "9:41".to_string()
}

/// Creates the page-dot row. The caller fills in the current page with
/// [update_page_dots].
fn make_page_dots(
    env: &mut Environment,
    main_view: id,
    screen_size: CGSize,
    page_count: usize,
) -> Vec<id> {
    let mut dots = Vec::with_capacity(page_count);
    // Old iOS only shows the indicator when there is more than one page.
    if page_count <= 1 {
        return dots;
    }
    let count = page_count as CGFloat;
    let total_width = PAGE_DOT_SIZE * count + PAGE_DOT_GAP * (count - 1.0);
    let first_x = (screen_size.width - total_width) / 2.0;
    let y = app_picker_dots_top(screen_size.height);
    let inactive = ui_color(env, 0.75, 0.75, 0.75, 0.45);
    for idx in 0..page_count {
        let dot = new_view(
            env,
            rect(
                first_x + idx as CGFloat * (PAGE_DOT_SIZE + PAGE_DOT_GAP),
                y,
                PAGE_DOT_SIZE,
                PAGE_DOT_SIZE,
            ),
            inactive,
        );
        let layer: id = msg![env; dot layer];
        () = msg![env; layer setCornerRadius:(PAGE_DOT_SIZE / 2.0)];
        () = msg![env; main_view addSubview:dot];
        dots.push(dot);
    }
    dots
}

/// Highlights the dot of the current page.
fn update_page_dots(env: &mut Environment, dots: &[id], current_page: usize) {
    let active: id = msg_class![env; UIColor whiteColor];
    let inactive = ui_color(env, 0.75, 0.75, 0.75, 0.45);
    for (idx, &dot) in dots.iter().enumerate() {
        let color = if idx == current_page {
            active
        } else {
            inactive
        };
        () = msg![env; dot setBackgroundColor:color];
    }
}

/// Re-creates the page dots after the number of pages changes.
fn rebuild_page_dots(
    env: &mut Environment,
    main_view: id,
    screen_size: CGSize,
    dots: &mut Vec<id>,
    page_count: usize,
) {
    for &dot in dots.iter() {
        () = msg![env; dot removeFromSuperview];
    }
    dots.clear();
    *dots = make_page_dots(env, main_view, screen_size, page_count);
}

const DOCK_ICON_SIZE: CGFloat = 60.0;
const DOCK_ICON_TOP: CGFloat = 5.0;
const DOCK_LABEL_HEIGHT: CGFloat = 12.0;

/// Builds the old-iOS dock at the bottom of the home screen. The shortcuts
/// live outside the paged grid, so they stay in place while pages slide.
fn make_dock(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
    recent_app: Option<(&str, id)>,
) -> id {
    let width = screen_size.width;
    let dock_inset = 6.0;
    let dock_width = width - dock_inset * 2.0;
    let top = screen_size.height - APP_PICKER_DOCK_HEIGHT + 2.0;
    let background = ui_color(env, 0.035, 0.045, 0.065, 0.62);
    let dock = new_view(
        env,
        rect(dock_inset, top, dock_width, APP_PICKER_DOCK_HEIGHT - 4.0),
        background,
    );
    let dock_layer: id = msg![env; dock layer];
    () = msg![env; dock_layer setCornerRadius:(12.0 as CGFloat)];
    let hairline_color = ui_color(env, 1.0, 1.0, 1.0, 0.38);
    let hairline = new_view(env, rect(12.0, 0.0, dock_width - 24.0, 1.0), hairline_color);
    () = msg![env; dock addSubview:hairline];

    let count: CGFloat = 4.0;
    let side_inset: CGFloat = 8.0;
    let gap =
        ((dock_width - side_inset * 2.0 - DOCK_ICON_SIZE * count) / (count - 1.0)).clamp(2.0, 18.0);
    let first_x = (dock_width - (DOCK_ICON_SIZE * count + gap * (count - 1.0))) / 2.0;

    let plus_image = make_icon_from_text(env, "+", 50.0, -6.0, (0.25, 0.25, 0.25, 1.0));
    let settings_image = make_settings_app_icon(env);
    let recent_placeholder = make_icon_from_text(env, "↻", 34.0, 0.0, (0.25, 0.25, 0.25, 1.0));
    let browse_image = make_browse_app_icon(env);
    let step = DOCK_ICON_SIZE + gap;
    make_dock_item(
        env, delegate, dock, first_x, plus_image, "Add .ipa", "addIpa",
    );
    make_dock_item(
        env,
        delegate,
        dock,
        first_x + step,
        settings_image,
        "Settings",
        "settingsShow",
    );
    let (recent_label, recent_image) = recent_app
        .map(|(name, image)| (name, image))
        .unwrap_or(("Recent app", recent_placeholder));
    make_dock_item(
        env,
        delegate,
        dock,
        first_x + step * 3.0,
        recent_image,
        recent_label,
        "openRecentApp",
    );
    make_dock_item(
        env,
        delegate,
        dock,
        first_x + step * 2.0,
        browse_image,
        "Browse",
        "addIpa",
    );
    () = msg![env; main_view addSubview:dock];
    dock
}

/// Adds one icon + label shortcut to the dock.
fn make_dock_item(
    env: &mut Environment,
    delegate: id,
    dock: id,
    x: CGFloat,
    image: id,
    label_text: &str,
    action: &str,
) {
    let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; icon_button setFrame:(rect(
        x,
        DOCK_ICON_TOP,
        DOCK_ICON_SIZE,
        DOCK_ICON_SIZE
    ))];
    () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
    let image_view: id = msg![env; icon_button imageView];
    () = msg![env; image_view setFrame:(rect(0.0, 0.0, DOCK_ICON_SIZE, DOCK_ICON_SIZE))];
    let layer: id = msg![env; image_view layer];
    let gravity = ns_string::get_static_str(env, "resizeAspect");
    () = msg![env; layer setContentsGravity:gravity];
    let selector = env.objc.lookup_selector(action).unwrap();
    () = msg![env; icon_button addTarget:delegate
                                  action:selector
                        forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dock addSubview:icon_button];

    let label_width = DOCK_ICON_SIZE + 30.0;
    let white: id = msg_class![env; UIColor whiteColor];
    let label = new_label(
        env,
        rect(
            x - (label_width - DOCK_ICON_SIZE) / 2.0,
            DOCK_ICON_TOP + DOCK_ICON_SIZE + 3.0,
            label_width,
            DOCK_LABEL_HEIGHT,
        ),
        label_text,
        10.5,
        true,
        white,
    );
    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.65);
    () = msg![env; label setShadowColor:shadow];
    () = msg![env; label setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; dock addSubview:label];
}

/// Helper to build a `CGRect` from its components.
fn rect(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

fn ui_color(env: &mut Environment, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) -> id {
    let color: id = msg_class![env; UIColor colorWithRed:r green:g blue:b alpha:a];
    color
}

/// Creates a plain `UIView` with the given frame and background colour.
fn new_view(env: &mut Environment, frame: CGRect, bg: id) -> id {
    let view: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view initWithFrame:frame];
    () = msg![env; view setBackgroundColor:bg];
    view
}

/// Creates a transparent `UILabel` with the given text and style.
fn new_label(
    env: &mut Environment,
    frame: CGRect,
    text: &str,
    font_size: CGFloat,
    bold: bool,
    text_color: id,
) -> id {
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    let ns_text = ns_string::from_rust_string(env, text.to_string());
    () = msg![env; label setText:ns_text];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
    () = msg![env; label setFont:font];
    () = msg![env; label setTextColor:text_color];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    label
}

fn new_setting_description_label(
    env: &mut Environment,
    frame: CGRect,
    text: &str,
    text_color: id,
) -> id {
    let label = new_label(
        env,
        frame,
        text,
        SETTINGS_DESCRIPTION_FONT_SIZE,
        false,
        text_color,
    );
    () = msg![env; label setTextAlignment:UITextAlignmentLeft];
    () = msg![env; label setLineBreakMode:UILineBreakModeWordWrap];
    () = msg![env; label setNumberOfLines:0];
    label
}

fn new_setting_title_label(
    env: &mut Environment,
    frame: CGRect,
    text: &str,
    font_size: CGFloat,
    text_color: id,
) -> id {
    let label = new_label(env, frame, text, font_size, false, text_color);
    () = msg![env; label setTextAlignment:UITextAlignmentLeft];
    () = msg![env; label setLineBreakMode:UILineBreakModeWordWrap];
    () = msg![env; label setNumberOfLines:0];
    label
}

fn set_view_x(env: &mut Environment, view: id, x: CGFloat) {
    let mut frame: CGRect = msg![env; view frame];
    frame.origin.x = x;
    () = msg![env; view setFrame:frame];
}

fn lerp_f(a: CGFloat, b: CGFloat, t: CGFloat) -> CGFloat {
    a + (b - a) * t
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Duration of the iOS 4-style radial icon fly-in when the picker opens.
const HOME_UNLOCK_DURATION: Duration = Duration::from_millis(600);
/// Initial scale of each icon and label at the start of the unlock fly-in.
const HOME_UNLOCK_START_SCALE: CGFloat = 1.65;
/// Fraction of the unlock duration by which the outermost corner icons are
/// staggered behind the innermost icons.
const HOME_UNLOCK_MAX_STAGGER: f64 = 0.22;
/// Base off-screen push distance as a fraction of the longer screen dimension,
/// ensuring even the innermost cells start outside the screen bounds.
const HOME_UNLOCK_BASE_PUSH_FACTOR: CGFloat = 0.55;
/// Extra radial distance multiplier applied to each cell's offset from the
/// grid center so outer rows/columns stay separated from inner ones in flight.
const HOME_UNLOCK_RADIAL_MULT: CGFloat = 1.85;
/// Duration of the page slide after a swipe or arrow-key press.
const PAGE_SLIDE_DURATION: Duration = Duration::from_millis(320);
/// Duration of the "app icon zooms to the middle, screen fades to black"
/// animation played when an app is picked.
/// Duration of the icon's move to the middle of the screen.
const APP_LAUNCH_MOVE_DURATION: Duration = Duration::from_millis(600);
/// How long the icon rests in the middle before it dissolves.
const APP_LAUNCH_CENTER_HOLD_DURATION: Duration = Duration::from_millis(400);
/// Duration of the dissolve of the icon into the black screen.
const APP_LAUNCH_DISSOLVE_DURATION: Duration = Duration::from_millis(500);
/// How long the black screen stays up before the app takes over.
const APP_LAUNCH_BLACK_HOLD_DURATION: Duration = Duration::from_millis(250);
/// Side length of the app icon at the centre of the launch animation.
const APP_LAUNCH_ICON_SIZE: CGFloat = 120.0;
/// How long an app icon must be held down before its delete dialog opens.
const APP_LONG_PRESS_DURATION: Duration = Duration::from_millis(500);

/// Runs the main run loop for `duration`, calling `step` with the linear
/// progress (0.0 to 1.0) before every iteration. Taps delivered while the
/// animation runs are dropped, because they would land on moving views.
fn animate_for(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    duration: Duration,
    mut step: impl FnMut(&mut Environment, f64),
) {
    let start = Instant::now();
    loop {
        let t = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
        step(env, t);
        run_run_loop_single_iteration(env, run_loop);
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
        if t >= 1.0 {
            break;
        }
    }
}

/// Precomputed trajectory of one icon-grid cell for the iOS 4-style home
/// screen unlock animation.
#[derive(Clone, Copy, Debug)]
struct UnlockCellTarget {
    /// Initial X displacement (off-screen) relative to the resting slot.
    start_dx: CGFloat,
    /// Initial Y displacement (off-screen) relative to the resting slot.
    start_dy: CGFloat,
    /// Resting vertical distance from the icon center to its label center,
    /// so the label stays attached below the scaled icon during flight.
    label_offset_y: CGFloat,
    /// Normalized radial distance from the grid center (`0.0..=1.0`), used
    /// to stagger outer icons slightly behind inner ones.
    norm_dist: f64,
}

fn compute_unlock_cell_targets(
    icon_centers: &[CGPoint],
    label_offsets_y: &[CGFloat],
    screen_size: CGSize,
) -> Vec<UnlockCellTarget> {
    if icon_centers.is_empty() {
        return Vec::new();
    }
    let mut min_x = icon_centers[0].x;
    let mut max_x = icon_centers[0].x;
    let mut min_y = icon_centers[0].y;
    let mut max_y = icon_centers[0].y;
    for c in icon_centers {
        min_x = min_x.min(c.x);
        max_x = max_x.max(c.x);
        min_y = min_y.min(c.y);
        max_y = max_y.max(c.y);
    }
    let grid_center = CGPoint {
        x: (min_x + max_x) / 2.0,
        y: (min_y + max_y) / 2.0,
    };

    let mut max_dist: CGFloat = 0.0;
    let mut raw: Vec<(CGFloat, CGFloat, CGFloat)> = Vec::with_capacity(icon_centers.len());
    for c in icon_centers {
        let vx = c.x - grid_center.x;
        let vy = c.y - grid_center.y;
        let dist = (vx * vx + vy * vy).sqrt();
        max_dist = max_dist.max(dist);
        raw.push((vx, vy, dist));
    }

    let base_push = screen_size.width.max(screen_size.height) * HOME_UNLOCK_BASE_PUSH_FACTOR;
    raw.into_iter()
        .zip(label_offsets_y.iter().copied())
        .map(|((vx, vy, dist), label_offset_y)| {
            let (ux, uy) = if dist > 1e-3 {
                (vx / dist, vy / dist)
            } else {
                (0.0, -1.0)
            };
            let start_dx = ux * base_push + vx * HOME_UNLOCK_RADIAL_MULT;
            let start_dy = uy * base_push + vy * HOME_UNLOCK_RADIAL_MULT;
            let norm_dist = if max_dist > 1e-3 {
                (dist / max_dist) as f64
            } else {
                0.0
            };
            UnlockCellTarget {
                start_dx,
                start_dy,
                label_offset_y,
                norm_dist,
            }
        })
        .collect()
}

/// Returns `(dx, dy, label_dy, scale, alpha)` for `target` at global
/// animation progress `t` (`0.0..=1.0`).
fn unlock_cell_state_at(
    target: &UnlockCellTarget,
    t: f64,
) -> (CGFloat, CGFloat, CGFloat, CGFloat, CGFloat) {
    let t = t.clamp(0.0, 1.0);
    let delay = target.norm_dist * HOME_UNLOCK_MAX_STAGGER;
    let span = (1.0 - HOME_UNLOCK_MAX_STAGGER).max(1e-6);
    let local_t = ((t - delay) / span).clamp(0.0, 1.0);
    let p = ease_out_cubic(local_t) as CGFloat;
    let rem = 1.0 - p;
    let dx = target.start_dx * rem;
    let dy = target.start_dy * rem;
    let scale = lerp_f(HOME_UNLOCK_START_SCALE, 1.0, p);
    let label_dy = dy + target.label_offset_y * (scale - 1.0);
    let alpha = (local_t * 2.0).min(1.0) as CGFloat;
    (dx, dy, label_dy, scale, alpha)
}

fn apply_unlock_step(
    env: &mut Environment,
    cells: &[(id, id)],
    targets: &[UnlockCellTarget],
    wallpaper: Option<id>,
    chrome_views: &[id],
    t: f64,
) {
    let chrome_alpha = ease_out_cubic(t.clamp(0.0, 1.0)) as CGFloat;
    if let Some(wallpaper) = wallpaper {
        () = msg![env; wallpaper setAlpha:chrome_alpha];
    }
    for &view in chrome_views {
        () = msg![env; view setAlpha:chrome_alpha];
    }

    for (&(icon_button, label), target) in cells.iter().zip(targets.iter()) {
        let (dx, dy, label_dy, scale, alpha) = unlock_cell_state_at(target, t);
        let icon_transform = CGAffineTransform::make_scale(scale, scale)
            .concat(CGAffineTransform::make_translation(dx, dy));
        let label_transform = CGAffineTransform::make_scale(scale, scale)
            .concat(CGAffineTransform::make_translation(dx, label_dy));
        () = msg![env; icon_button setTransform:icon_transform];
        () = msg![env; icon_button setAlpha:alpha];
        () = msg![env; label setTransform:label_transform];
        () = msg![env; label setAlpha:alpha];
    }
}

/// Plays the iOS 4-style home screen unlock animation when the picker opens:
/// app icons and their labels fly in radially from outside the screen into
/// their grid slots, with inner icons leading and outer corners settling
/// right after.
#[allow(clippy::too_many_arguments)]
fn play_home_screen_unlock_animation(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    grid: &IconGridStuff,
    screen_size: CGSize,
    wallpaper: Option<id>,
    chrome_views: &[id],
) {
    let cells = grid.containers[grid.visible].cells.clone();
    let mut icon_centers = Vec::with_capacity(cells.len());
    let mut label_offsets_y = Vec::with_capacity(cells.len());
    for &(icon_button, label) in &cells {
        let icon_frame: CGRect = msg![env; icon_button frame];
        let label_frame: CGRect = msg![env; label frame];
        let icon_center = CGPoint {
            x: icon_frame.origin.x + icon_frame.size.width / 2.0,
            y: icon_frame.origin.y + icon_frame.size.height / 2.0,
        };
        let label_center_y = label_frame.origin.y + label_frame.size.height / 2.0;
        icon_centers.push(icon_center);
        label_offsets_y.push(label_center_y - icon_center.y);
    }
    let targets = compute_unlock_cell_targets(&icon_centers, &label_offsets_y, screen_size);

    // Render the initial off-screen state once before starting the timer so
    // first-frame texture creation doesn't eat into the animation duration.
    apply_unlock_step(env, &cells, &targets, wallpaper, chrome_views, 0.0);
    run_run_loop_single_iteration(env, run_loop);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
    }

    animate_for(env, run_loop, delegate, HOME_UNLOCK_DURATION, |env, t| {
        apply_unlock_step(env, &cells, &targets, wallpaper, chrome_views, t);
    });

    if let Some(wallpaper) = wallpaper {
        () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
    }
    for &view in chrome_views {
        () = msg![env; view setAlpha:(1.0 as CGFloat)];
    }
    for &(icon_button, label) in &cells {
        () = msg![env; icon_button setTransform:CGAffineTransformIdentity];
        () = msg![env; icon_button setAlpha:(1.0 as CGFloat)];
        () = msg![env; label setTransform:CGAffineTransformIdentity];
        () = msg![env; label setAlpha:(1.0 as CGFloat)];
    }
}

/// Plays the iOS-style launch animation: the tapped icon travels to the middle
/// of the screen while the screen fades to black. The app then starts on top.
fn play_app_launch_animation(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
    icon_frame: CGRect,
    icon: id,
) {
    let black: id = msg_class![env; UIColor blackColor];
    let overlay = new_view(
        env,
        rect(0.0, 0.0, screen_size.width, screen_size.height),
        black,
    );
    () = msg![env; overlay setAlpha:(0.0 as CGFloat)];
    () = msg![env; main_view addSubview:overlay];

    let icon_view: id = msg_class![env; UIImageView alloc];
    let icon_view: id = msg![env; icon_view initWithImage:icon];
    () = msg![env; icon_view setFrame:icon_frame];
    () = msg![env; main_view addSubview:icon_view];

    let target_x = (screen_size.width - APP_LAUNCH_ICON_SIZE) / 2.0;
    let target_y = (screen_size.height - APP_LAUNCH_ICON_SIZE) / 2.0;
    // 1. The icon travels to the middle of the screen and grows.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_MOVE_DURATION,
        |env, t| {
            let p = ease_in_out_cubic(t.min(1.0)) as CGFloat;
            let frame = rect(
                lerp_f(icon_frame.origin.x, target_x, p),
                lerp_f(icon_frame.origin.y, target_y, p),
                lerp_f(icon_frame.size.width, APP_LAUNCH_ICON_SIZE, p),
                lerp_f(icon_frame.size.height, APP_LAUNCH_ICON_SIZE, p),
            );
            () = msg![env; icon_view setFrame:frame];
            // The black background fades in behind the moving icon.
            () = msg![env; overlay setAlpha:(p as CGFloat)];
        },
    );
    // 2. The icon rests in the middle for a moment.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_CENTER_HOLD_DURATION,
        |_env, _t| {},
    );
    // 3. The icon dissolves into the black background.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_DISSOLVE_DURATION,
        |env, t| {
            let p = ease_in_out_cubic(t.min(1.0));
            () = msg![env; icon_view setAlpha:((1.0 - p) as CGFloat)];
        },
    );
    // 4. Hold on the black screen before the app takes over.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_BLACK_HOLD_DURATION,
        |_env, _t| {},
    );
}

/// Slides the icon grid to `new_page`. `direction` is +1.0 when moving to a
/// later page (the new page comes in from the right) and -1.0 for an earlier
/// one. The two page containers swap roles at the end.
fn slide_to_page(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    grid: &mut IconGridStuff,
    apps: &mut [AppInfo],
    new_page: usize,
    direction: CGFloat,
) {
    let front = grid.visible;
    let back = front ^ 1;
    let width = grid.screen_width;
    update_icon_grid(env, grid, apps, new_page, back);

    let front_view = grid.containers[front].view;
    let back_view = grid.containers[back].view;
    set_view_x(env, back_view, direction * width);
    () = msg![env; back_view setHidden:false];

    animate_for(env, run_loop, delegate, PAGE_SLIDE_DURATION, |env, t| {
        let p = ease_out_cubic(t) as CGFloat;
        set_view_x(env, front_view, -direction * width * p);
        set_view_x(env, back_view, direction * width * (1.0 - p));
    });

    set_view_x(env, front_view, 0.0);
    () = msg![env; front_view setHidden:true];
    grid.visible = back;
}

/// Returns `(scale, alpha)` at progress `t` (`0.0..=1.0`) for the centered
/// pop-in (`open == true`) or pop-out (`open == false`) animation shared by
/// the Delete dialog and the Settings screen.
fn pop_dialog_state_at(t: f64, open: bool) -> (CGFloat, CGFloat) {
    let t = t.clamp(0.0, 1.0);
    let visible = if open {
        ease_out_cubic(t)
    } else {
        1.0 - ease_out_cubic(t)
    };
    let scale = DELETE_DIALOG_START_SCALE + (1.0 - DELETE_DIALOG_START_SCALE) * visible;
    (scale as CGFloat, visible as CGFloat)
}

/// Hides or restores Settings scroll rows that sit outside `main_view` while
/// `main_view` is scaled below full size, since the compositor does not clip
/// `UIScrollView` children to its bounds.
fn set_offscreen_scroll_items_hidden(
    env: &mut Environment,
    settings: &SettingsStuff,
    hide_offscreen: bool,
) {
    if !hide_offscreen {
        for &(view, _, _) in &settings.scroll_items {
            () = msg![env; view setHidden:false];
        }
        return;
    }
    let scroll_view = settings.scroll_view;
    let offset: CGPoint = msg![env; scroll_view contentOffset];
    let offset_y = offset.y;
    let top_limit = settings.scroll_origin_y;
    let bottom_limit = settings.full_height - 20.0;
    for &(view, top_y, bottom_y) in &settings.scroll_items {
        let top_in_settings = settings.scroll_origin_y + top_y - offset_y;
        let bottom_in_settings = settings.scroll_origin_y + bottom_y - offset_y;
        let offscreen = bottom_in_settings <= top_limit || top_in_settings >= bottom_limit;
        () = msg![env; view setHidden:offscreen];
    }
}

/// Puts the settings scrollbar in step with `scroll_view`: the thumb spans the
/// part of the list that is on screen and moves down the track as it scrolls.
fn update_settings_scrollbar(env: &mut Environment, settings: &SettingsStuff) {
    let settings_view = settings.main_view;
    let hidden: bool = msg![env; settings_view isHidden];
    if hidden {
        return;
    }
    let scroll = settings.scroll_view;
    let bounds: CGRect = msg![env; scroll bounds];
    let content_size: CGSize = msg![env; scroll contentSize];
    let offset: CGPoint = msg![env; scroll contentOffset];
    let list_height = settings.list_height;
    let content_height = content_size.height.max(bounds.size.height);
    // A list that fits needs no thumb: the track alone says there is nothing
    // more to scroll.
    if content_height - list_height < 1.0 {
        () = msg![env; (settings.scrollbar_thumb) setHidden:true];
        return;
    }
    () = msg![env; (settings.scrollbar_thumb) setHidden:false];
    let thumb_height = (list_height * list_height / content_height).max(48.0);
    let travel = (list_height - thumb_height).max(0.0);
    let scrolled = (offset.y / (content_height - list_height)).clamp(0.0, 1.0);
    let thumb_frame = CGRect {
        origin: CGPoint {
            x: settings.scrollbar_x,
            y: settings.list_top + scrolled * travel,
        },
        size: CGSize {
            width: SETTINGS_SCROLLBAR_WIDTH,
            height: thumb_height,
        },
    };
    () = msg![env; (settings.scrollbar_thumb) setFrame:thumb_frame];
}
/// Opens the Settings screen with the same centered pop-in animation as
/// [show_delete_dialog]: the screen dims while the view grows from the center
/// and fades in over [DELETE_DIALOG_APPEAR_DURATION].
fn show_settings_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    settings: &SettingsStuff,
) {
    let dimmer = settings.dimmer;
    let settings_view = settings.main_view;
    let (start_scale, _) = pop_dialog_state_at(0.0, true);
    let initial_transform = CGAffineTransform::make_scale(start_scale, start_scale);
    () = msg![env; dimmer setAlpha:(0.0 as CGFloat)];
    () = msg![env; dimmer setHidden:false];
    () = msg![env; settings_view setTransform:initial_transform];
    () = msg![env; settings_view setAlpha:(0.0 as CGFloat)];
    () = msg![env; settings_view setHidden:false];

    // Render one invisible frame before starting the timer so first-time
    // texture creation for Settings rows doesn't eat into the pop-in duration.
    run_run_loop_single_iteration(env, run_loop);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }

    set_offscreen_scroll_items_hidden(env, settings, true);
    update_settings_scrollbar(env, settings);
    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_APPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, true);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; settings_view setTransform:transform];
            () = msg![env; settings_view setAlpha:alpha];
            () = msg![env; dimmer setAlpha:alpha];
        },
    );
    () = msg![env; settings_view setTransform:CGAffineTransformIdentity];
    () = msg![env; settings_view setAlpha:(1.0 as CGFloat)];
    () = msg![env; dimmer setHidden:true];
    set_offscreen_scroll_items_hidden(env, settings, false);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }
}

/// Closes the Settings screen with the same centered pop-out animation as
/// [hide_delete_dialog].
fn hide_settings_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    settings: &SettingsStuff,
) {
    let dimmer = settings.dimmer;
    let settings_view = settings.main_view;
    () = msg![env; dimmer setAlpha:(1.0 as CGFloat)];
    () = msg![env; dimmer setHidden:false];
    set_offscreen_scroll_items_hidden(env, settings, true);

    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_DISAPPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, false);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; settings_view setTransform:transform];
            () = msg![env; settings_view setAlpha:alpha];
            () = msg![env; dimmer setAlpha:alpha];
        },
    );
    () = msg![env; dimmer setHidden:true];
    () = msg![env; settings_view setHidden:true];
    () = msg![env; settings_view setTransform:CGAffineTransformIdentity];
    () = msg![env; settings_view setAlpha:(1.0 as CGFloat)];
    set_offscreen_scroll_items_hidden(env, settings, false);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }
}

/// Loads a flat monochrome dock icon, applying the same rounded mask and
/// subtle gloss as the original gray picker icons.
fn make_dock_asset_icon(env: &mut Environment, bytes: &[u8]) -> id {
    let mut image = Image::from_bytes(bytes).unwrap();
    image.round_icon_corners(/* add_sheen: */ true);
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

fn make_settings_app_icon(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/res/settings_icon.png"
    ));
    make_dock_asset_icon(env, bytes)
}

fn make_browse_app_icon(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/browse_icon.png"));
    make_dock_asset_icon(env, bytes)
}

/// Blue for the selected segment, light grey for the others (iOS style).
fn update_segment_buttons(env: &mut Environment, buttons: &[id], selected_idx: usize) {
    for (idx, &button) in buttons.iter().enumerate() {
        let selected = idx == selected_idx;
        let bg = if selected {
            ui_color(env, 0.0, 0.478, 1.0, 1.0)
        } else {
            ui_color(env, 0.87, 0.87, 0.89, 1.0)
        };
        let title_color: id = if selected {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor blackColor]
        };
        () = msg![env; button setBackgroundColor:bg];
        () = msg![env; button setTitleColor:title_color forState:UIControlStateNormal];
    }
}

fn update_scale_hack_buttons(env: &mut Environment, buttons: &[id], value: Option<NonZeroU32>) {
    update_segment_buttons(env, buttons, value.map_or(0, |v| v.get() as usize));
}

fn update_orientation_buttons(
    env: &mut Environment,
    buttons: &[id],
    value: Option<DeviceOrientation>,
) {
    update_segment_buttons(
        env,
        buttons,
        value.map_or(0, |v| match v {
            DeviceOrientation::LandscapeLeft => 1,
            DeviceOrientation::LandscapeRight => 2,
            DeviceOrientation::PortraitUpsideDown => 3,
            _ => panic!(),
        }),
    );
}

fn update_performance_overlay_buttons(
    env: &mut Environment,
    mode_buttons: &[id],
    corner_buttons: &[id],
    mode: PerformanceOverlayMode,
    corner: PerformanceOverlayCorner,
) {
    update_segment_buttons(
        env,
        mode_buttons,
        match mode {
            PerformanceOverlayMode::Full => 0,
            PerformanceOverlayMode::Minimal => 1,
            PerformanceOverlayMode::TitleBar => 2,
        },
    );
    update_segment_buttons(
        env,
        corner_buttons,
        match corner {
            PerformanceOverlayCorner::TopLeft => 0,
            PerformanceOverlayCorner::TopRight => 1,
            PerformanceOverlayCorner::BottomRight => 2,
            PerformanceOverlayCorner::BottomLeft => 3,
        },
    );
}

/// Creates a horizontal row of segment-style buttons inside `parent`.
fn make_segment_buttons(
    env: &mut Environment,
    delegate: id,
    parent: id,
    frame: CGRect,
    buttons: &[(&'static str, &'static str)],
) -> Vec<id> {
    let gap: CGFloat = 4.0;
    let count = buttons.len() as CGFloat;
    let button_width = (frame.size.width - gap * (count - 1.0)) / count;
    let mut result = Vec::new();
    for (i, &(title_text, selector)) in buttons.iter().enumerate() {
        let button_frame = rect(
            frame.origin.x + (i as CGFloat) * (button_width + gap),
            frame.origin.y,
            button_width,
            frame.size.height,
        );
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; button setFrame:button_frame];
        let text = ns_string::get_static_str(env, title_text);
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        // FIXME: manually calling layoutSubviews shouldn't be needed?
        () = msg![env; button layoutSubviews];
        let label: id = msg![env; button titleLabel];
        let font: id = msg_class![env; UIFont systemFontOfSize:(13.0 as CGFloat)];
        () = msg![env; label setFont:font];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        () = msg![env; label setLineBreakMode:UILineBreakModeWordWrap];
        () = msg![env; label setNumberOfLines:2];
        let layer: id = msg![env; button layer];
        () = msg![env; layer setCornerRadius:(5.0 as CGFloat)];
        let selector = env.objc.lookup_selector(selector).unwrap();
        () = msg![env; button addTarget:delegate
                                 action:selector
                       forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; parent addSubview:button];
        result.push(button);
    }
    result
}

/// One row of the settings screen.
#[derive(Clone)]
enum SettingsRow {
    /// A title, description, and row of segment buttons.
    Segmented(
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    ),
    /// The "Device model" row (its dropdown is a root-level overlay).
    DeviceDropdown(&'static str, &'static str),
    /// The "Language" row (its dropdown is a root-level overlay).
    LanguageDropdown(&'static str, &'static str),
    /// Title and description with switch: selector, initial state, enabled.
    Toggle(&'static str, &'static str, &'static str, bool, bool),
    /// Renderer mode dropdown.
    RendererDropdown(&'static str, &'static str),
    /// A full-width action row with centered red text, like the old-iOS
    /// "Delete Account" cell.
    Button {
        title: &'static str,
        selector: &'static str,
    },
    /// A single-line value committed when its text field resigns first responder.
    ManualValue {
        title: String,
        description: String,
        tag: NSInteger,
        value: String,
        enabled: bool,
        text_keyboard: bool,
    },
}

const SETTINGS_NAV_BAR_HEIGHT: CGFloat = 44.0;
const SETTINGS_TOP_PADDING: CGFloat = 12.0;
const SETTINGS_GROUP_INSET: CGFloat = 16.0;
const SETTINGS_ROW_SIDE_INSET: CGFloat = 16.0;
const SETTINGS_DESCRIPTION_FONT_SIZE: CGFloat = 11.0;
const SETTINGS_HEADER_HEIGHT: CGFloat = 28.0;
const SETTINGS_SECTION_GAP: CGFloat = 10.0;
/// Width of the dropdown lists (list plus scrollbar).
const SETTINGS_DROPDOWN_MENU_WIDTH: CGFloat = 280.0;
const MANUAL_DEADZONE: NSInteger = 1001;
const MANUAL_X_TILT_RANGE: NSInteger = 1002;
const MANUAL_Y_TILT_RANGE: NSInteger = 1003;
const MANUAL_X_TILT_OFFSET: NSInteger = 1004;
const MANUAL_Y_TILT_OFFSET: NSInteger = 1005;
const MANUAL_BUTTON_TOUCH_BASE: NSInteger = 1100;
const MANUAL_DPAD_TOUCH: NSInteger = 1007;
const MANUAL_STICK_TOUCH: NSInteger = 1008;
const MANUAL_FPS_LIMIT: NSInteger = 1009;
const MANUAL_CORRUPTION_INTENSITY: NSInteger = 1010;
const MANUAL_CORRUPTION_MAX_OFFSET: NSInteger = 1011;
const MANUAL_CORRUPTION_SEED: NSInteger = 1012;

fn settings_text_height(
    env: &mut Environment,
    text: &str,
    font_size: CGFloat,
    width: CGFloat,
) -> CGFloat {
    let string = ns_string::from_rust_string(env, text.to_string());
    let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
    let size: CGSize = msg![env; string sizeWithFont:font
                                      constrainedToSize:(CGSize { width, height: CGFloat::MAX })
                                          lineBreakMode:UILineBreakModeWordWrap];
    release(env, string);
    size.height
}

fn settings_row_height(env: &mut Environment, row: &SettingsRow, inner_width: CGFloat) -> CGFloat {
    let side = SETTINGS_ROW_SIDE_INSET;
    match row {
        SettingsRow::Segmented(title, description, _) => {
            let text_width = inner_width - 2.0 * side;
            8.0 + settings_text_height(env, *title, 15.0, text_width)
                + 4.0
                + settings_text_height(
                    env,
                    *description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 8.0
                + 24.0
                + 8.0
        }
        SettingsRow::Toggle(title, description, _, _, _) => {
            let text_width = inner_width - 2.0 * side - 94.0 - 8.0;
            (settings_text_height(env, *title, 16.0, text_width)
                + settings_text_height(
                    env,
                    *description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 12.0)
                .max(27.0)
                + 16.0
        }
        SettingsRow::DeviceDropdown(title, description) => {
            let text_width = inner_width - 2.0 * side - 190.0 - 8.0;
            (settings_text_height(env, *title, 14.0, text_width)
                + settings_text_height(
                    env,
                    *description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 12.0)
                .max(30.0)
                + 16.0
        }
        SettingsRow::LanguageDropdown(title, description) => {
            let text_width = inner_width - 2.0 * side - 170.0 - 8.0;
            (settings_text_height(env, *title, 16.0, text_width)
                + settings_text_height(
                    env,
                    *description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 12.0)
                .max(30.0)
                + 16.0
        }
        SettingsRow::RendererDropdown(title, description) => {
            let text_width = inner_width - 2.0 * side - 130.0 - 8.0;
            (settings_text_height(env, *title, 14.0, text_width)
                + settings_text_height(
                    env,
                    *description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 12.0)
                .max(30.0)
                + 16.0
        }
        SettingsRow::Button { title, .. } => {
            settings_text_height(env, title, 16.0, inner_width - 2.0 * side) + 24.0
        }
        SettingsRow::ManualValue {
            title, description, ..
        } => {
            let text_width = inner_width - 2.0 * side - 128.0 - 12.0;
            (settings_text_height(env, title, 15.0, text_width)
                + settings_text_height(
                    env,
                    description,
                    SETTINGS_DESCRIPTION_FONT_SIZE,
                    text_width,
                )
                + 12.0)
                .max(42.0)
                + 16.0
        }
    }
}

/// The views of the Settings screen that the event loop needs to update.
struct SettingsStuff {
    /// Dims the picker behind the Settings screen while it pops in or out.
    dimmer: id,
    /// Full-screen settings view (hidden until the Settings tile is tapped).
    main_view: id,
    /// Scrollable list of grouped rows below the navigation bar.
    scroll_view: id,
    /// `(view, top_y, bottom_y)` for each child added directly to
    /// `scroll_view`, used to hide off-screen rows while scaled.
    scroll_items: Vec<(id, CGFloat, CGFloat)>,
    /// Total height of `main_view`, including the status bar strip.
    full_height: CGFloat,
    /// Vertical origin of `scroll_view` inside `main_view`.
    scroll_origin_y: CGFloat,
    /// The thumb of the scrollbar down the right edge of the settings list,
    /// moved to follow `scroll_view`'s offset.
    scrollbar_thumb: id,
    /// Left edge of the scrollbar, which is in `main_view` coordinates.
    scrollbar_x: CGFloat,
    /// Top and height of `scroll_view` inside `main_view`.
    list_top: CGFloat,
    list_height: CGFloat,
    scale_hack_buttons: [id; 5],
    orientation_buttons: [id; 4],
    overlay_mode_buttons: Vec<id>,
    overlay_corner_buttons: Vec<id>,
    corruption_mode_buttons: Vec<id>,
    corruption_startup_delay_buttons: Vec<id>,
    corruption_cooldown_buttons: Vec<id>,
    gles2_mode_buttons: Vec<id>,
    vsync_buttons: Vec<id>,
    present_mode_buttons: Vec<id>,
    texture_filter_buttons: Vec<id>,
    present_rotate_buttons: Vec<id>,
    controller_to_touch_buttons: Vec<id>,
    /// `(selector, switch)` for each toggle row, so a reset can put every
    /// switch back where the new settings say it should be.
    toggle_switches: Vec<(&'static str, id)>,
    manual_value_fields: Vec<(NSInteger, id, id)>,
    /// The "Device model" dropdown.
    device_dropdown: SettingsDropdown,
    /// The "Language" dropdown.
    language_dropdown: SettingsDropdown,
    /// The "Renderer Mode" dropdown.
    renderer_dropdown: SettingsDropdown,
}

/// The views of one settings dropdown: a button in a settings row, and a list
/// that opens over the whole Settings screen when the button is tapped.
struct SettingsDropdown {
    /// The button in the settings row. Its title shows the selection.
    button: id,
    /// The list (hidden until the button is tapped).
    menu: id,
    /// Scrollable list content inside `menu`.
    scroll_view: id,
    /// Dims the screen behind the open list; tapping it closes the list.
    dimmer: id,
    /// One button per choice, in the order of the spec's entries.
    items: Vec<id>,
    /// The scrollbar thumb shown alongside the list.
    thumb: id,
}

/// What a settings dropdown offers, and which delegate selectors its buttons
/// call (the `&'static str` names are looked up when the views are built).
struct DropdownSpec {
    /// `(title, tag)` for each choice, in display order.
    entries: Vec<(String, NSInteger)>,
    toggle_selector: &'static str,
    item_selector: &'static str,
    scroll_selector: &'static str,
    button_font_size: CGFloat,
}

/// The state that the picker keeps for one settings dropdown.
#[derive(Clone, Default)]
struct DropdownState {
    /// The tag of the chosen entry, or `None` for the default.
    selected: Option<i32>,
    /// Whether the list is currently open.
    open: bool,
    /// Index of the first visible entry in the list.
    scroll: isize,
}

fn set_dropdown_open(env: &mut Environment, menu: id, dimmer: id, open: bool) {
    () = msg![env; menu setHidden:(!open)];
    () = msg![env; dimmer setHidden:(!open)];
}

/// Makes the dropdown match `state`: the list open or closed, the selection
/// highlighted, the list scrolled, and the button titled with `label`.
fn refresh_dropdown(
    env: &mut Environment,
    dropdown: &SettingsDropdown,
    label: &str,
    state: &DropdownState,
) {
    set_dropdown_open(env, dropdown.menu, dropdown.dimmer, state.open);
    let offset = state.scroll as CGFloat * DEVICE_MENU_ITEM_HEIGHT;
    let content_offset = CGPoint { x: 0.0, y: offset };
    () = msg![env; (dropdown.scroll_view) setContentOffset:content_offset];
    update_dropdown_menu(
        env,
        dropdown.scroll_view,
        &dropdown.items,
        dropdown.thumb,
        state.selected,
    );
    let arrow = if state.open { "▲" } else { "▼" };
    let title = ns_string::from_rust_string(env, format!("{label} {arrow}"));
    () = msg![env; (dropdown.button) setTitle:title forState:UIControlStateNormal];
    release(env, title);
}

/// The largest scroll offset for a list of `item_count` entries.
fn dropdown_max_scroll(item_count: usize) -> isize {
    (item_count as isize - DEVICE_MENU_VISIBLE_ITEMS as isize).max(0)
}

fn dropdown_scroll_position(env: &mut Environment, sender: id, event: id) -> Option<CGFloat> {
    let touches: id = msg![env; event touchesForView:sender];
    let touch: id = msg![env; touches anyObject];
    (touch != nil).then(|| {
        let point: CGPoint = msg![env; touch locationInView:sender];
        point.y
    })
}

fn dropdown_scroll_at(position: CGFloat, item_count: usize) -> isize {
    let travel = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT - 54.0;
    (((position - 27.0) / travel) * dropdown_max_scroll(item_count) as CGFloat)
        .round()
        .clamp(0.0, dropdown_max_scroll(item_count) as CGFloat) as isize
}

fn sync_dropdown_scroll(
    env: &mut Environment,
    dropdown: &SettingsDropdown,
    state: &mut DropdownState,
) {
    if !state.open {
        return;
    }
    let offset: CGPoint = msg![env; (dropdown.scroll_view) contentOffset];
    state.scroll = ((offset.y / DEVICE_MENU_ITEM_HEIGHT).round() as isize)
        .clamp(0, dropdown_max_scroll(dropdown.items.len()));
    update_dropdown_scroll_position(env, dropdown.scroll_view, &dropdown.items, dropdown.thumb);
}

/// Builds the Settings screen: a navigation bar with a "Done" button, and a
/// scrollable list of grouped sections in the style of the iOS Settings app.
fn setup_settings(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
    picker_settings: &PickerSettings,
    renderer_mode_dropdown_enabled: bool,
    show_fps: bool,
    show_fullscreen_row: bool,
) -> SettingsStuff {
    let width = app_frame.size.width;
    let height = app_frame.size.height;
    let (manual_screen_width, manual_screen_height) =
        picker_touch_screen_size(&env.options, picker_settings);
    // The Settings screen covers the whole window, including the status bar
    // strip above the application frame. Nothing is clipped by the emulator,
    // so scrolled content must not be able to show up in that strip.
    let status_offset = app_frame.origin.y;
    let full_height = height + status_offset;
    let full_frame = rect(0.0, -status_offset, width, full_height);

    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.4);
    let dimmer = new_view(env, full_frame, dim_color);
    () = msg![env; dimmer setHidden:true];
    () = msg![env; super_view addSubview:dimmer];

    let grouped_bg = ui_color(env, 0.79, 0.82, 0.85, 1.0);
    let settings_view = new_view(env, full_frame, grouped_bg);
    () = msg![env; settings_view setHidden:true];
    () = msg![env; super_view addSubview:settings_view];
    let stripe = ui_color(env, 0.55, 0.58, 0.62, 0.10);
    let mut stripe_x: CGFloat = 2.0;
    while stripe_x < width {
        let stripe_view = new_view(
            env,
            rect(stripe_x, 0.0, 1.0, height + status_offset),
            stripe,
        );
        () = msg![env; settings_view addSubview:stripe_view];
        stripe_x += 5.0;
    }

    let clear: id = msg_class![env; UIColor clearColor];
    let black: id = msg_class![env; UIColor blackColor];
    let white: id = msg_class![env; UIColor whiteColor];
    let manual_value_background = ui_color(env, 0.94, 0.94, 0.96, 1.0);
    let gray_text = ui_color(env, 0.43, 0.43, 0.45, 1.0);
    // Navigation bar with the title and a "Done" button.
    let nav_bar_bg = ui_color(env, 0.20, 0.34, 0.52, 1.0);
    let nav_bar = new_view(
        env,
        rect(0.0, 0.0, width, SETTINGS_NAV_BAR_HEIGHT + status_offset),
        nav_bar_bg,
    );
    () = msg![env; settings_view addSubview:nav_bar];
    let nav_height = SETTINGS_NAV_BAR_HEIGHT + status_offset;
    const NAV_GRADIENT_STEPS: usize = 48;
    for step in 0..NAV_GRADIENT_STEPS {
        let t = step as CGFloat / (NAV_GRADIENT_STEPS - 1) as CGFloat;
        let y = nav_height * t;
        let band_height = nav_height / NAV_GRADIENT_STEPS as CGFloat + 0.5;
        let color = ui_color(env, 0.39 - 0.18 * t, 0.56 - 0.25 * t, 0.75 - 0.23 * t, 1.0);
        let band = new_view(env, rect(0.0, y, width, band_height), color);
        () = msg![env; nav_bar addSubview:band];
    }
    let nav_separator_color = ui_color(env, 0.12, 0.22, 0.36, 1.0);
    let nav_separator = new_view(
        env,
        rect(
            0.0,
            SETTINGS_NAV_BAR_HEIGHT + status_offset - 1.0,
            width,
            1.0,
        ),
        nav_separator_color,
    );
    () = msg![env; settings_view addSubview:nav_separator];

    let title_color = ui_color(env, 1.0, 1.0, 1.0, 1.0);
    let title = new_label(
        env,
        rect(60.0, status_offset, width - 120.0, SETTINGS_NAV_BAR_HEIGHT),
        "Settings",
        17.0,
        true,
        title_color,
    );
    () = msg![env; title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; settings_view addSubview:title];

    let done: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () =
        msg![env; done setFrame:(rect(width - 72.0, status_offset, 64.0, SETTINGS_NAV_BAR_HEIGHT))];
    let done_text = ns_string::get_static_str(env, "Done");
    () = msg![env; done setTitle:done_text forState:UIControlStateNormal];
    // FIXME: manually calling layoutSubviews shouldn't be needed?
    () = msg![env; done layoutSubviews];
    () = msg![env; done setTitleColor:white forState:UIControlStateNormal];
    let done_label: id = msg![env; done titleLabel];
    let done_font: id = msg_class![env; UIFont boldSystemFontOfSize:(17.0 as CGFloat)];
    () = msg![env; done_label setFont:done_font];
    let hide_selector = env.objc.lookup_selector("settingsHide").unwrap();
    () = msg![env; done addTarget:delegate
                           action:hide_selector
                 forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; settings_view addSubview:done];

    // Scrollable content below the navigation bar.
    let scroll: id = msg_class![env; UIScrollView alloc];
    let scroll: id = msg![env; scroll initWithFrame:(rect(
        0.0,
        SETTINGS_NAV_BAR_HEIGHT + status_offset,
        width,
        height - SETTINGS_NAV_BAR_HEIGHT,
    ))];
    () = msg![env; scroll setBackgroundColor:clear];
    // Clip the scrolled content so it never slides up over the navigation bar.
    // The list also stops short of the scrollbar, so rows never draw over it.
    () = msg![env; scroll setClipsToBounds:true];
    () = msg![env; scroll setFrame:(rect(
        0.0,
        SETTINGS_NAV_BAR_HEIGHT + status_offset,
        width - SETTINGS_SCROLLBAR_WIDTH,
        height - SETTINGS_NAV_BAR_HEIGHT,
    ))];
    () = msg![env; settings_view addSubview:scroll];
    // A scrollbar down the right edge, like the ones beside the dropdown
    // lists, so a long list of settings shows where you are in it.
    let list_top = SETTINGS_NAV_BAR_HEIGHT + status_offset;
    let list_height = height - SETTINGS_NAV_BAR_HEIGHT;
    let scrollbar_x = width - SETTINGS_SCROLLBAR_WIDTH;
    // Built in locals: `new_view` borrows `env`, so `ui_color` can't be called
    // in its argument list.
    let track_color = ui_color(env, 0.55, 0.58, 0.62, 0.18);
    let thumb_color = ui_color(env, 0.30, 0.32, 0.36, 0.75);
    let track = new_view(
        env,
        rect(scrollbar_x, list_top, SETTINGS_SCROLLBAR_WIDTH, list_height),
        track_color,
    );
    () = msg![env; settings_view addSubview:track];
    let scrollbar_thumb = new_view(
        env,
        rect(scrollbar_x, list_top, SETTINGS_SCROLLBAR_WIDTH, 0.0),
        thumb_color,
    );
    () = msg![env; settings_view addSubview:scrollbar_thumb];
    () = msg![env; settings_view bringSubviewToFront:scrollbar_thumb];
    // The navigation bar must stay on top of the scrolled content.
    () = msg![env; settings_view bringSubviewToFront:nav_bar];
    () = msg![env; settings_view bringSubviewToFront:nav_separator];
    () = msg![env; settings_view bringSubviewToFront:title];
    () = msg![env; settings_view bringSubviewToFront:done];

    let mut sections: Vec<(&'static str, Vec<SettingsRow>)> = Vec::new();
    let mut display_rows = vec![
        SettingsRow::Segmented(
            "Scale hack",
            "Renders the game at a larger internal size before scaling it to your screen. This can make edges look sharper, but may slow the game or cause graphics problems.",
            &[
                ("Default", "scaleHackDefault"),
                ("Off", "scaleHack1"),
                ("2×", "scaleHack2"),
                ("3×", "scaleHack3"),
                ("4×", "scaleHack4"),
            ],
        ),
        SettingsRow::Segmented(
            "Orientation",
            "Choose how the game screen is turned. Default lets the game choose; the arrows force a direction if the game starts sideways.",
            &[
                ("Default", "orientationDefault"),
                ("←", "orientationLandscapeLeft"),
                ("→", "orientationLandscapeRight"),
                ("↓", "orientationPortraitUpsideDown"),
            ],
        ),
        SettingsRow::Segmented(
            "VSync",
            "Wait for the display before showing each frame. Default: Auto (platform default).",
            &[("Auto", "vsyncAuto"), ("On", "vsyncOn"), ("Off", "vsyncOff")],
        ),
    ];
    if show_fullscreen_row {
        display_rows.push(SettingsRow::Toggle(
            "Fullscreen (override)",
            "Choose whether the game fills the screen. This overrides the app’s saved setting.",
            "fullscreen:",
            picker_settings.fullscreen,
            true,
        ));
    }
    sections.push(("DISPLAY", display_rows));
    let overlay_mode_options: &'static [(&'static str, &'static str)] =
        if cfg!(any(target_os = "android", target_os = "ios")) {
            &[
                ("Full", "overlayModeFull"),
                ("Minimal", "overlayModeMinimal"),
            ]
        } else {
            &[
                ("Full", "overlayModeFull"),
                ("Minimal", "overlayModeMinimal"),
                ("Title bar", "overlayModeTitleBar"),
            ]
        };
    let mut graphics_rows = vec![
        SettingsRow::RendererDropdown(
            "Renderer Mode",
            "Android only. ANGLE translates the game’s graphics calls; Native GLES uses your device’s graphics driver. If one has problems, try the other.",
        ),
        SettingsRow::Segmented(
            "GLES2 compatibility",
            "Changes how ES 1.1/2.0 graphics are handled. Prefer Context upgrades an app’s ES 1.1 request to ES 2.0; GL2.1 uses an older, incomplete compatibility backend. Either choice can break games.",
            &[
                ("Off", "gles2Off"),
                ("Prefer context", "gles2Prefer"),
                ("GL2.1", "gles2Compatibility"),
            ],
        ),
        SettingsRow::Toggle(
            "Force Composition",
            "Sends the game’s image through an extra layer before display. This may help some graphics problems, but can reduce speed.",
            "forceComposition:",
            picker_settings.force_composition,
            true,
        ),
        SettingsRow::Toggle(
            "Performance display",
            "Shows performance information such as FPS while the game runs.",
            "showFPS:",
            show_fps,
            true,
        ),
        SettingsRow::Toggle(
            "Trace GL errors",
            "Logs where OpenGL errors occur. Diagnostic only: checking errors changes what the game receives from glGetError().",
            "traceGLErrors:",
            picker_settings.trace_gl_errors,
            true,
        ),
        SettingsRow::Toggle(
            "Trace GL draws",
            "Logs draw state for debugging. May create large logs and cache up to 16 MiB.",
            "traceGLDraws:",
            picker_settings.trace_gl_draws,
            true,
        ),
        SettingsRow::Segmented(
            "Presentation mode",
            "Auto chooses a compatible path; Direct is faster; Readback is slower but can help broken drivers. Default: Auto.",
            &[
                ("Auto", "presentModeAuto"),
                ("Direct", "presentModeDirect"),
                ("Readback", "presentModeReadback"),
            ],
        ),
        SettingsRow::Segmented(
            "Texture min-filter fix",
            "Overrides incomplete mipmap filtering for level-zero textures. Default: App default.",
            &[
                ("App default", "textureFilterDefault"),
                ("On", "textureFilterOn"),
                ("Off", "textureFilterOff"),
            ],
        ),
        SettingsRow::Segmented(
            "Presentation rotation",
            "Force the displayed image rotation. Default: App default.",
            &[
                ("App", "presentRotateDefault"),
                ("0°", "presentRotate0"),
                ("90°", "presentRotate90"),
                ("180°", "presentRotate180"),
                ("270°", "presentRotate270"),
            ],
        ),
        manual_value_row(
            picker_settings,
            MANUAL_FPS_LIMIT,
            "FPS limit".to_string(),
            manual_screen_width,
            manual_screen_height,
            true,
            false,
        ),
        SettingsRow::Toggle(
            "Log FPS to console",
            "Prints the current frame rate once per second. Default: Off.",
            "printFPS:",
            picker_settings.print_fps,
            true,
        ),
        SettingsRow::Toggle(
            "Error popups",
            "Show error dialogs or log errors only. Default: Show.",
            "popupErrors:",
            picker_settings.popup_errors,
            true,
        ),
        SettingsRow::Segmented(
            "Display mode",
            "Choose how much performance information appears on screen. Title bar is only useful on desktop.",
            overlay_mode_options,
        ),
        SettingsRow::Segmented(
            "Screen corner",
            "Choose where the on-screen performance display appears.",
            &[
                ("Top\nLeft", "overlayCornerTopLeft"),
                ("Top\nRight", "overlayCornerTopRight"),
                ("Bottom\nRight", "overlayCornerBottomRight"),
                ("Bottom\nLeft", "overlayCornerBottomLeft"),
            ],
        ),
        // Both fixes are opt-in and may be enabled per-app in the options
        // files. Leaving a switch alone keeps those defaults; flipping it
        // (either way) is an explicit override for every later launch.
        SettingsRow::Toggle(
            "Rotation fix",
            "Uses UIKit’s actual screen rotation to orient the game image. This may help some iPad games; it can change how other games turn.",
            "rotationFix:",
            picker_settings.rotation_fix.unwrap_or(false),
            true,
        ),
        SettingsRow::Toggle(
            "Movie player fix",
            "Changes how in-game videos are loaded and shown. It may help some games, but can stop videos from appearing in others.",
            "moviePlayerFix:",
            picker_settings.fix_movie_player.unwrap_or(false),
            true,
        ),
    ];
    graphics_rows.push(SettingsRow::Toggle(
        "Show slide to unlock",
        "Shows the old iPhone lock screen before the game starts.",
        "showLockScreen:",
        picker_settings.show_lock_screen,
        true,
    ));
    if show_fullscreen_row {
        graphics_rows.push(SettingsRow::Toggle(
            "Keep aspect ratio on resize",
            "On desktop, keep the game’s proportions when resizing the window instead of stretching the image.",
            "preserveAspectRatio:",
            picker_settings.preserve_aspect_ratio,
            true,
        ));
    }
    sections.push(("GRAPHICS", graphics_rows));
    let mut input_rows = vec![
        SettingsRow::Segmented(
            "Controller to touch",
            "Default: App profile (off unless it defines mappings). On uses the settings below; Off disables profile mappings.",
            &[
                ("App\ndefault", "controllerToTouchDefault"),
                ("On", "controllerToTouchOn"),
                ("Off", "controllerToTouchOff"),
            ],
        ),
        manual_value_row(
            picker_settings,
            MANUAL_DEADZONE,
            "Controller deadzone".to_string(),
            manual_screen_width,
            manual_screen_height,
            manual_value_enabled(MANUAL_DEADZONE, picker_settings),
            true,
        ),
        manual_value_row(
            picker_settings,
            MANUAL_X_TILT_RANGE,
            "Tilt X range".to_string(),
            manual_screen_width,
            manual_screen_height,
            picker_settings.analog_stick_tilt_controls,
            false,
        ),
        manual_value_row(
            picker_settings,
            MANUAL_Y_TILT_RANGE,
            "Tilt Y range".to_string(),
            manual_screen_width,
            manual_screen_height,
            picker_settings.analog_stick_tilt_controls,
            false,
        ),
        manual_value_row(
            picker_settings,
            MANUAL_X_TILT_OFFSET,
            "Tilt X offset".to_string(),
            manual_screen_width,
            manual_screen_height,
            picker_settings.analog_stick_tilt_controls,
            false,
        ),
        manual_value_row(
            picker_settings,
            MANUAL_Y_TILT_OFFSET,
            "Tilt Y offset".to_string(),
            manual_screen_width,
            manual_screen_height,
            picker_settings.analog_stick_tilt_controls,
            false,
        ),
    ];
    for button in CONTROLLER_BUTTONS {
        input_rows.push(manual_value_row(
            picker_settings,
            manual_button_tag(button),
            format!("{} touch point", button_name(button)),
            manual_screen_width,
            manual_screen_height,
            manual_value_enabled(manual_button_tag(button), picker_settings),
            false,
        ));
    }
    input_rows.push(manual_value_row(
        picker_settings,
        MANUAL_DPAD_TOUCH,
        "D-pad touch area".to_string(),
        manual_screen_width,
        manual_screen_height,
        manual_value_enabled(MANUAL_DPAD_TOUCH, picker_settings),
        false,
    ));
    input_rows.push(manual_value_row(
        picker_settings,
        MANUAL_STICK_TOUCH,
        "Stick touch area".to_string(),
        manual_screen_width,
        manual_screen_height,
        manual_value_enabled(MANUAL_STICK_TOUCH, picker_settings),
        false,
    ));
    sections.push(("INPUT", input_rows));
    sections.push((
        "CORRUPTION",
        {
            let mut rows = vec![
            SettingsRow::Segmented(
                "Corruption engine",
                "Intentionally changes a few bytes in the game’s live memory. This experimental effect can cause glitches or crashes; it is off by default.",
                &[("Off", "corruptionOff"), ("On", "corruptionOn")],
            ),
            SettingsRow::Segmented(
                "Startup delay",
                "Wait this long after the game first runs before corruption starts. The timer pauses while the game is not running.",
                &[("0s", "corruptionStartupImmediate"), ("5s", "corruptionStartup5"), ("15s", "corruptionStartup15"), ("30s", "corruptionStartup30")],
            ),
            SettingsRow::Segmented(
                "Cooldown",
                "Wait this long between corruption bursts while the game is running.",
                &[("0.5s", "corruptionCooldownHalf"), ("1s", "corruptionCooldown1"), ("5s", "corruptionCooldown5"), ("10s", "corruptionCooldown10")],
            ),
            ];
            let enabled = picker_settings.corruption_mode == CorruptionMode::Corrupt;
            rows.extend([
                manual_value_row(
                    picker_settings,
                    MANUAL_CORRUPTION_INTENSITY,
                    "Bytes per burst".to_string(),
                    manual_screen_width,
                    manual_screen_height,
                    enabled,
                    true,
                ),
                manual_value_row(
                    picker_settings,
                    MANUAL_CORRUPTION_MAX_OFFSET,
                    "Maximum allocation offset".to_string(),
                    manual_screen_width,
                    manual_screen_height,
                    enabled,
                    false,
                ),
                manual_value_row(
                    picker_settings,
                    MANUAL_CORRUPTION_SEED,
                    "Random seed".to_string(),
                    manual_screen_width,
                    manual_screen_height,
                    enabled,
                    true,
                ),
            ]);
            rows
        },
    ));
    sections.push((
        "DEVICE",
        vec![
            SettingsRow::DeviceDropdown(
                "Device model",
                "Choose which iPhone, iPad, or iPod touch model the game sees. Some games only support certain models.",
            ),
            SettingsRow::LanguageDropdown(
                "Language",
                "Choose the language order reported to the game. The game decides which languages it supports.",
            ),
        ],
    ));
    sections.push((
        "EMULATION",
        vec![
        SettingsRow::Button {
            title: "Reset all settings",
            selector: "resetSettingsTapped",
        },
            SettingsRow::Toggle(
                "Cheat Engine",
                "Enables the built-in cheat interface for supported games. It does not add cheats to every game.",
                "cheatEngine:",
                picker_settings.cheat_engine,
                true,
            ),
            SettingsRow::Toggle(
                "Network access",
                "Lets the game connect to the Internet and local network. Online services may still be unavailable.",
                "network:",
                picker_settings.network,
                true,
            ),
            SettingsRow::Toggle(
                "Local Game Center sign-in",
                "Tells the game that a local player is signed in. Online Game Center services are not provided.",
                "gameCenterAuthenticated:",
                picker_settings.game_center_authenticated,
                true,
            ),
            SettingsRow::Toggle(
                "Use analog sticks for tilt controls",
                "Uses a controller’s analog stick to imitate tilting the device. When enabled, the real accelerometer is ignored.",
                "analogStickTiltControls:",
                picker_settings.analog_stick_tilt_controls,
                true,
            ),
        ],
    ));

    let inner_width = width - 2.0 * SETTINGS_GROUP_INSET;
    let side = SETTINGS_ROW_SIDE_INSET;
    let mut scale_hack_buttons: Vec<id> = Vec::new();
    let mut orientation_buttons: Vec<id> = Vec::new();
    let mut device_dropdown: Option<SettingsDropdown> = None;
    let mut language_dropdown: Option<SettingsDropdown> = None;
    let mut renderer_dropdown: Option<SettingsDropdown> = None;
    let mut overlay_mode_buttons = Vec::new();
    let mut overlay_corner_buttons = Vec::new();
    let mut corruption_mode_buttons = Vec::new();
    let mut corruption_startup_delay_buttons = Vec::new();
    let mut corruption_cooldown_buttons = Vec::new();
    let mut gles2_mode_buttons = Vec::new();
    let mut vsync_buttons = Vec::new();
    let mut present_mode_buttons = Vec::new();
    let mut texture_filter_buttons = Vec::new();
    let mut present_rotate_buttons = Vec::new();
    let mut controller_to_touch_buttons = Vec::new();
    let mut toggle_switches: Vec<(&'static str, id)> = Vec::new();
    let mut manual_value_fields = Vec::new();
    let mut scroll_items: Vec<(id, CGFloat, CGFloat)> = Vec::new();
    // Where dropdown menus open: just under the navigation bar.
    let menu_origin = CGPoint {
        x: (width - SETTINGS_DROPDOWN_MENU_WIDTH) / 2.0,
        y: SETTINGS_NAV_BAR_HEIGHT + status_offset + 12.0,
    };

    let mut y: CGFloat = SETTINGS_TOP_PADDING;
    for (header, rows) in sections {
        let header_label = new_label(
            env,
            rect(SETTINGS_GROUP_INSET + 12.0, y, inner_width - 12.0, 18.0),
            header,
            13.0,
            false,
            gray_text,
        );
        () = msg![env; scroll addSubview:header_label];
        scroll_items.push((header_label, y, y + 18.0));
        y += SETTINGS_HEADER_HEIGHT;

        let row_heights: Vec<CGFloat> = rows
            .iter()
            .map(|row| settings_row_height(env, row, inner_width))
            .collect();
        let group_height: CGFloat = row_heights.iter().sum();
        let outline_color = ui_color(env, 0.66, 0.67, 0.69, 1.0);
        let outline = new_view(
            env,
            rect(
                SETTINGS_GROUP_INSET - 1.0,
                y - 1.0,
                inner_width + 2.0,
                group_height + 2.0,
            ),
            outline_color,
        );
        let outline_layer: id = msg![env; outline layer];
        () = msg![env; outline_layer setCornerRadius:(11.0 as CGFloat)];
        () = msg![env; scroll addSubview:outline];
        scroll_items.push((outline, y, y + group_height));
        let group = new_view(
            env,
            rect(SETTINGS_GROUP_INSET, y, inner_width, group_height),
            white,
        );
        let group_layer: id = msg![env; group layer];
        () = msg![env; group_layer setCornerRadius:(10.0 as CGFloat)];
        () = msg![env; scroll addSubview:group];
        scroll_items.push((group, y, y + group_height));

        let row_count = rows.len();
        let mut row_y: CGFloat = 0.0;
        for (index, row) in rows.iter().enumerate() {
            let row_height = row_heights[index];
            match row.clone() {
                SettingsRow::Segmented(title, description, buttons) => {
                    let text_width = inner_width - 2.0 * side;
                    let title_height = settings_text_height(env, title, 15.0, text_width);
                    let description_height = settings_text_height(
                        env,
                        description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        text_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, text_width, title_height),
                        title,
                        15.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            text_width,
                            description_height,
                        ),
                        description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];
                    let buttons_y = row_y + 8.0 + title_height + 4.0 + description_height + 8.0;
                    let seg_buttons = make_segment_buttons(
                        env,
                        delegate,
                        group,
                        rect(side, buttons_y, text_width, 24.0),
                        buttons,
                    );
                    match buttons[0].1 {
                        "scaleHackDefault" => scale_hack_buttons = seg_buttons,
                        "orientationDefault" => orientation_buttons = seg_buttons,
                        "overlayModeFull" => overlay_mode_buttons = seg_buttons,
                        "overlayCornerTopLeft" => overlay_corner_buttons = seg_buttons,
                        "corruptionOff" => corruption_mode_buttons = seg_buttons,
                        "corruptionStartupImmediate" => {
                            corruption_startup_delay_buttons = seg_buttons
                        }
                        "corruptionCooldownHalf" => corruption_cooldown_buttons = seg_buttons,
                        "gles2Off" => gles2_mode_buttons = seg_buttons,
                        "vsyncAuto" => vsync_buttons = seg_buttons,
                        "presentModeAuto" => present_mode_buttons = seg_buttons,
                        "textureFilterDefault" => texture_filter_buttons = seg_buttons,
                        "presentRotateDefault" => present_rotate_buttons = seg_buttons,
                        "controllerToTouchDefault" => controller_to_touch_buttons = seg_buttons,
                        _ => unreachable!(),
                    }
                }
                SettingsRow::DeviceDropdown(title, description) => {
                    let dropdown_width: CGFloat = 190.0;
                    let label_width = (inner_width - 2.0 * side - dropdown_width - 8.0).max(0.0);
                    let title_height = settings_text_height(env, title, 14.0, label_width);
                    let description_height = settings_text_height(
                        env,
                        description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        label_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, label_width, title_height),
                        title,
                        14.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            label_width,
                            description_height,
                        ),
                        description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];
                    device_dropdown = Some(make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(
                            inner_width - side - dropdown_width,
                            row_y + (row_height - 30.0) / 2.0,
                            dropdown_width,
                            30.0,
                        ),
                        menu_origin,
                        device_model_dropdown_spec(),
                    ));
                }
                SettingsRow::LanguageDropdown(title, description) => {
                    let dropdown_width: CGFloat = 170.0;
                    let label_width = inner_width - 2.0 * side - dropdown_width - 8.0;
                    let title_height = settings_text_height(env, title, 16.0, label_width);
                    let description_height = settings_text_height(
                        env,
                        description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        label_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, label_width, title_height),
                        title,
                        16.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            label_width,
                            description_height,
                        ),
                        description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];
                    language_dropdown = Some(make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(
                            inner_width - side - dropdown_width,
                            row_y + (row_height - 30.0) / 2.0,
                            dropdown_width,
                            30.0,
                        ),
                        menu_origin,
                        language_dropdown_spec(),
                    ));
                }
                SettingsRow::RendererDropdown(title, description) => {
                    let dropdown_width: CGFloat = 130.0;
                    let label_width = inner_width - 2.0 * side - dropdown_width - 8.0;
                    let title_height = settings_text_height(env, title, 14.0, label_width);
                    let description_height = settings_text_height(
                        env,
                        description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        label_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, label_width, title_height),
                        title,
                        14.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            label_width,
                            description_height,
                        ),
                        description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];
                    let dropdown = make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(
                            inner_width - side - dropdown_width,
                            row_y + (row_height - 30.0) / 2.0,
                            dropdown_width,
                            30.0,
                        ),
                        menu_origin,
                        renderer_mode_dropdown_spec(),
                    );
                    () = msg![env; (dropdown.button) setEnabled:renderer_mode_dropdown_enabled];
                    renderer_dropdown = Some(dropdown);
                }
                SettingsRow::Toggle(title, description, selector_name, default_state, enabled) => {
                    // Keep every switch the same size; let long labels wrap
                    // within the remaining row width instead of shrinking the
                    // control or drawing text underneath it.
                    let switch_width: CGFloat = 94.0;
                    let text_width = inner_width - 2.0 * side - switch_width - 8.0;
                    let title_height = settings_text_height(env, title, 16.0, text_width);
                    let description_height = settings_text_height(
                        env,
                        description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        text_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, text_width, title_height),
                        title,
                        16.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            text_width,
                            description_height,
                        ),
                        description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];

                    let switch: id = msg_class![env; UISwitch alloc];
                    let switch: id = msg![env; switch initWithFrame:(rect(
                        inner_width - side - switch_width,
                        row_y + (row_height - 27.0) / 2.0,
                        switch_width,
                        27.0,
                    ))];
                    () = msg![env; switch setOn:default_state];
                    () = msg![env; switch setEnabled:enabled];
                    let selector = env.objc.lookup_selector(selector_name).unwrap();
                    () = msg![env; switch addTarget:delegate
                                             action:selector
                                   forControlEvents:UIControlEventValueChanged];
                    () = msg![env; group addSubview:switch];
                    toggle_switches.push((selector_name, switch));
                }
                SettingsRow::Button { title, selector } => {
                    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
                    () = msg![env; button setFrame:(rect(side, row_y, inner_width - 2.0 * side, row_height))];
                    let clear: id = msg_class![env; UIColor clearColor];
                    () = msg![env; button setBackgroundColor:clear];
                    let text = ns_string::from_rust_string(env, title.to_string());
                    () = msg![env; button setTitle:text forState:UIControlStateNormal];
                    release(env, text);
                    let red = ui_color(env, 0.88, 0.18, 0.20, 1.0);
                    () = msg![env; button setTitleColor:red forState:UIControlStateNormal];
                    // FIXME: manually calling layoutSubviews shouldn't be
                    // needed?
                    () = msg![env; button layoutSubviews];
                    let label: id = msg![env; button titleLabel];
                    let font: id = msg_class![env; UIFont systemFontOfSize:(16.0 as CGFloat)];
                    () = msg![env; label setFont:font];
                    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
                    let selector = env.objc.lookup_selector(selector).unwrap();
                    () = msg![env; button addTarget:delegate
                action:selector
                forControlEvents:UIControlEventTouchUpInside];
                    () = msg![env; group addSubview:button];
                }
                SettingsRow::ManualValue {
                    title,
                    description,
                    tag,
                    value,
                    enabled,
                    text_keyboard,
                } => {
                    let field_width: CGFloat = 120.0;
                    let text_width = inner_width - 2.0 * side - field_width - 20.0;
                    let title_height = settings_text_height(env, &title, 15.0, text_width);
                    let description_height = settings_text_height(
                        env,
                        &description,
                        SETTINGS_DESCRIPTION_FONT_SIZE,
                        text_width,
                    );
                    let title_view = new_setting_title_label(
                        env,
                        rect(side, row_y + 8.0, text_width, title_height),
                        &title,
                        15.0,
                        black,
                    );
                    () = msg![env; group addSubview:title_view];
                    let description_view = new_setting_description_label(
                        env,
                        rect(
                            side,
                            row_y + 8.0 + title_height + 4.0,
                            text_width,
                            description_height,
                        ),
                        &description,
                        gray_text,
                    );
                    () = msg![env; group addSubview:description_view];
                    let field: id = msg_class![env; UITextField alloc];
                    let field: id = msg![env; field initWithFrame:(rect(
                        inner_width - side - field_width,
                        row_y + (row_height - 32.0) / 2.0,
                        field_width,
                        32.0,
                    ))];
                    let value = ns_string::from_rust_string(env, value);
                    () = msg![env; field setText:value];
                    release(env, value);
                    () = msg![env; field setTextColor:black];
                    () = msg![env; field setTextAlignment:UITextAlignmentCenter];
                    () = msg![env; field setBackgroundColor:manual_value_background];
                    let field_font_size: CGFloat = if tag == MANUAL_CORRUPTION_SEED {
                        10.5
                    } else {
                        13.0
                    };
                    let field_font: id = msg_class![env; UIFont systemFontOfSize:field_font_size];
                    () = msg![env; field setFont:field_font];
                    () = msg![env; field setTag:tag];
                    () = msg![env; field setDelegate:delegate];
                    () = msg![env; field setEnabled:enabled];
                    () = msg![env; field setKeyboardType:(
                        if text_keyboard {
                            UIKeyboardTypeNumbersAndPunctuation
                        } else {
                            UIKeyboardTypeDefault
                        }
                    )];
                    () = msg![env; field setReturnKeyType:UIReturnKeyDone];
                    // The field's internal label otherwise starts at zero size.
                    () = msg![env; field layoutSubviews];
                    () = msg![env; group addSubview:field];
                    manual_value_fields.push((tag, field, description_view));
                }
            }
            row_y += row_height;
            // Hairline separator between rows, inset from the left like iOS.
            if index + 1 < row_count {
                let separator_color = ui_color(env, 0.85, 0.85, 0.87, 1.0);
                let separator = new_view(
                    env,
                    rect(side, row_y - 1.0, inner_width - side, 1.0),
                    separator_color,
                );
                () = msg![env; group addSubview:separator];
            }
        }
        y += group_height + SETTINGS_SECTION_GAP;
    }

    // Footer with the build identifier.
    let footer = new_label(
        env,
        rect(0.0, y, width, 18.0),
        &format!("{} ({})", crate::DISPLAY_NAME, crate::COMMIT_HASH),
        12.0,
        false,
        gray_text,
    );
    () = msg![env; footer setTextAlignment:UITextAlignmentCenter];
    () = msg![env; scroll addSubview:footer];
    scroll_items.push((footer, y, y + 18.0));
    y += 30.0;

    () = msg![env; scroll setContentSize:(CGSize { width, height: y + 24.0 })];

    SettingsStuff {
        dimmer,
        main_view: settings_view,
        scroll_view: scroll,
        scroll_items,
        full_height,
        scroll_origin_y: SETTINGS_NAV_BAR_HEIGHT + status_offset,
        scrollbar_thumb,
        scrollbar_x,
        list_top,
        list_height,
        scale_hack_buttons: scale_hack_buttons.try_into().unwrap(),
        orientation_buttons: orientation_buttons.try_into().unwrap(),
        overlay_mode_buttons,
        overlay_corner_buttons,
        corruption_mode_buttons,
        corruption_startup_delay_buttons,
        corruption_cooldown_buttons,
        gles2_mode_buttons,
        vsync_buttons,
        present_mode_buttons,
        texture_filter_buttons,
        present_rotate_buttons,
        controller_to_touch_buttons,
        toggle_switches,
        manual_value_fields,
        device_dropdown: device_dropdown
            .expect("the settings screen always has a device model row"),
        language_dropdown: language_dropdown
            .expect("the settings screen always has a language row"),
        renderer_dropdown: renderer_dropdown
            .expect("the settings screen always has a renderer mode row"),
    }
}

/// Sentinel button tags for the device-model dropdown. Model buttons use their
/// picker tag (`DeviceFamily::tag`, at most 20) as their tag, so the sentinels
/// are placed well above that range.
const DEVICE_TAG_DEFAULT: NSInteger = 1000;
const DEVICE_TAG_AUTO: NSInteger = 1001;

/// How many rows of the device-model dropdown are visible at once before the
/// list has to be scrolled.
const DEVICE_MENU_VISIBLE_ITEMS: usize = 6;
/// Height of a single row in the device-model dropdown.
const DEVICE_MENU_ITEM_HEIGHT: CGFloat = 30.0;
/// Width of the scrollbar down the right edge of the settings list.
const SETTINGS_SCROLLBAR_WIDTH: CGFloat = 10.0;

/// The choices shown in the device-model dropdown, in display order, as
/// `(title, tag)` pairs: "Default" (no override), "Auto" (match host screen),
/// then one entry per [crate::window::DeviceFamily] in `ALL_SELECTABLE` order.
/// Model titles include portrait pixel resolution and use each model's own
/// picker tag, so saved settings survive changes to the display order.
fn device_model_entries() -> Vec<(String, NSInteger)> {
    use crate::window::DeviceFamily;
    let mut entries: Vec<(String, NSInteger)> = Vec::new();
    entries.push(("Default".to_string(), DEVICE_TAG_DEFAULT));
    entries.push(("Auto".to_string(), DEVICE_TAG_AUTO));
    for family in DeviceFamily::ALL_SELECTABLE {
        let Some(tag) = family.tag() else {
            continue;
        };
        entries.push((device_model_resolution_label(family), tag as NSInteger));
    }
    entries
}

fn device_model_resolution_label(family: &crate::window::DeviceFamily) -> String {
    let (width, height) = family.portrait_size();
    let scale = family.scale_factor() as u32;
    format!(
        "{} ({}x{}px)",
        family.display_name(),
        width * scale,
        height * scale
    )
}

/// The spec of the "Device model" dropdown.
fn device_model_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: device_model_entries(),
        toggle_selector: "deviceModelToggle",
        item_selector: "deviceModel:",
        scroll_selector: "deviceModelScroll:forEvent:",
        button_font_size: 14.0,
    }
}

/// The languages offered by the "Language" dropdown, as `(title, value)`.
/// `value` is what `--preferred-languages=` gets. `None` means "Auto": no
/// override, so the game sees the host's own language settings. Only codes
/// that ns_bundle.rs maps to `.lproj` folders are listed, with English as a
/// fallback.
const LANGUAGE_CHOICES: &[(&str, Option<&str>)] = &[
    ("Auto", None),
    ("English", Some("en")),
    ("Русский", Some("ru,en")),
    ("Deutsch", Some("de,en")),
    ("Français", Some("fr,en")),
    ("Español", Some("es,en")),
    ("Italiano", Some("it,en")),
    ("Português", Some("pt,en")),
    ("Nederlands", Some("nl,en")),
    ("Svenska", Some("sv,en")),
    ("Dansk", Some("da,en")),
    ("Norsk", Some("no,en")),
    ("Suomi", Some("fi,en")),
    ("Türkçe", Some("tr,en")),
    ("日本語", Some("ja,en")),
    ("简体中文", Some("zh-Hans,zh,en")),
];

const RENDERER_MODE_ANGLE_TAG: NSInteger = 2000;
const RENDERER_MODE_NATIVE_GLES_TAG: NSInteger = 2001;

fn renderer_mode_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: vec![
            ("ANGLE".to_string(), RENDERER_MODE_ANGLE_TAG),
            ("Native GLES".to_string(), RENDERER_MODE_NATIVE_GLES_TAG),
        ],
        toggle_selector: "rendererModeToggle",
        item_selector: "rendererMode:",
        scroll_selector: "rendererModeScroll:forEvent:",
        button_font_size: 16.0,
    }
}

fn renderer_mode_tag(mode: RendererMode) -> i32 {
    match mode {
        RendererMode::Angle => RENDERER_MODE_ANGLE_TAG as i32,
        RendererMode::NativeGles => RENDERER_MODE_NATIVE_GLES_TAG as i32,
    }
}

fn renderer_mode_from_tag(tag: Option<i32>) -> RendererMode {
    match tag.map(|tag| tag as NSInteger) {
        Some(RENDERER_MODE_NATIVE_GLES_TAG) => RendererMode::NativeGles,
        _ => RendererMode::Angle,
    }
}

/// Human-readable renderer mode label for the selected dropdown item.
fn renderer_mode_label_for_tag(tag: Option<i32>) -> String {
    match renderer_mode_from_tag(tag) {
        RendererMode::Angle => "ANGLE",
        RendererMode::NativeGles => "Native GLES",
    }
    .to_string()
}

/// The spec of the "Language" dropdown.
fn language_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: LANGUAGE_CHOICES
            .iter()
            .enumerate()
            .map(|(idx, (title, _))| (title.to_string(), idx as NSInteger))
            .collect(),
        toggle_selector: "languageToggle",
        item_selector: "language:",
        scroll_selector: "languageScroll:forEvent:",
        button_font_size: 16.0,
    }
}

/// The button title of the language dropdown for the chosen tag.
fn language_label_for_tag(tag: Option<i32>) -> String {
    let idx = tag.unwrap_or(0) as usize;
    LANGUAGE_CHOICES
        .get(idx)
        .map_or("Auto", |choice| choice.0)
        .to_string()
}

/// The launch argument for the chosen language, or `None` for "Auto".
fn language_argument(tag: Option<i32>) -> Option<String> {
    let idx = tag?;
    let &(_, codes) = LANGUAGE_CHOICES.get(idx as usize)?;
    codes.map(|codes| format!("--preferred-languages={codes}"))
}

/// Human-readable label for a device-model choice tag, used as the dropdown
/// button title.
fn device_model_label_for_tag(tag: Option<i32>) -> String {
    use crate::window::DeviceFamily;
    match tag.map(|t| t as NSInteger) {
        None | Some(DEVICE_TAG_DEFAULT) => "Default".to_string(),
        Some(DEVICE_TAG_AUTO) => "Auto".to_string(),
        Some(idx) => DeviceFamily::for_tag(idx as usize)
            .map(|family| device_model_resolution_label(&family))
            .unwrap_or_else(|| "Default".to_string()),
    }
}

/// Re-lay-out and re-style the device-model dropdown list for the given scroll
/// offset and current selection. Items are positioned relative to `scroll`
/// (each row is `DEVICE_MENU_ITEM_HEIGHT` tall); rows outside the visible
/// window are hidden. The currently-selected item is highlighted in magenta,
/// the rest in dark gray. The scrollbar thumb is moved to reflect `scroll`.
fn update_dropdown_menu(
    env: &mut Environment,
    scroll_view: id,
    items: &[id],
    thumb: id,
    selected: Option<i32>,
) {
    update_dropdown_scroll_position(env, scroll_view, items, thumb);
    for &item in items {
        let tag: NSInteger = msg![env; item tag];
        let is_selected = selected.is_some_and(|v| v as NSInteger == tag);
        let color = if is_selected {
            ui_color(env, 0.08, 0.40, 0.78, 1.0)
        } else {
            ui_color(env, 1.0, 1.0, 1.0, 1.0)
        };
        let text_color: id = if is_selected {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor blackColor]
        };
        () = msg![env; item setBackgroundColor:color];
        () = msg![env; item setTitleColor:text_color forState:UIControlStateNormal];
    }
}

fn update_dropdown_scroll_position(
    env: &mut Environment,
    scroll_view: id,
    items: &[id],
    thumb: id,
) {
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
    let offset: CGPoint = msg![env; scroll_view contentOffset];
    for (j, &item) in items.iter().enumerate() {
        let item_top = j as CGFloat * DEVICE_MENU_ITEM_HEIGHT - offset.y;
        let visible = item_top + DEVICE_MENU_ITEM_HEIGHT > 0.0 && item_top < visible_menu_height;
        let hidden: bool = msg![env; item isHidden];
        let should_hide = !visible;
        if hidden != should_hide {
            () = msg![env; item setHidden:should_hide];
        }
    }
    let max_offset =
        ((items.len() as CGFloat) * DEVICE_MENU_ITEM_HEIGHT - visible_menu_height).max(0.0);
    let thumb_height: CGFloat = 54.0;
    let travel = (visible_menu_height - thumb_height).max(0.0);
    let thumb_y = if max_offset > 0.0 {
        (offset.y / max_offset).clamp(0.0, 1.0) * travel
    } else {
        0.0
    };
    let frame: CGRect = msg![env; thumb frame];
    if (frame.origin.y - thumb_y).abs() > 0.01 {
        let thumb_frame = CGRect {
            origin: CGPoint {
                x: 256.0,
                y: thumb_y,
            },
            size: CGSize {
                width: 24.0,
                height: thumb_height,
            },
        };
        () = msg![env; thumb setFrame:thumb_frame];
    }
}

/// Builds a settings dropdown: a button in a settings row, whose title shows
/// the selection, plus a list that is shown as an overlay on the whole Settings
/// screen, with a dimmer behind it. The list holds one button per entry of
/// `spec`, a scrollbar track and thumb, and a draggable control over them.
/// The list starts out hidden, and its titles are set by
/// [refresh_dropdown].
fn make_settings_dropdown(
    env: &mut Environment,
    delegate: id,
    group_view: id,
    root_view: id,
    button_frame: CGRect,
    menu_origin: CGPoint,
    spec: DropdownSpec,
) -> SettingsDropdown {
    let DropdownSpec {
        entries,
        toggle_selector,
        item_selector,
        scroll_selector,
        button_font_size,
    } = spec;
    let list_width: CGFloat = 256.0;
    let scrollbar_width: CGFloat = 24.0;
    let dark_gray: id = msg_class![env; UIColor darkGrayColor];
    let root_size: CGSize = {
        let bounds: CGRect = msg![env; root_view bounds];
        bounds.size
    };

    // Dimmer covering the whole Settings screen while the list is open.
    let dimmer: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; dimmer setFrame:(rect(0.0, 0.0, root_size.width, root_size.height))];
    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.35);
    () = msg![env; dimmer setBackgroundColor:dim_color];
    () = msg![env; dimmer addTarget:delegate
                             action:(env.objc.lookup_selector(toggle_selector).unwrap())
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dimmer setHidden:true];
    () = msg![env; root_view addSubview:dimmer];

    // The toggle button, shown in the settings row. Its title is set by
    // `refresh_dropdown`.
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let blue = ui_color(env, 0.0, 0.478, 1.0, 1.0);
    () = msg![env; button setTitleColor:blue forState:UIControlStateNormal];
    () = msg![env; button setFrame:button_frame];
    () = msg![env; button layoutSubviews];
    let button_label: id = msg![env; button titleLabel];
    let button_font: id = msg_class![env; UIFont systemFontOfSize:button_font_size];
    () = msg![env; button_label setFont:button_font];
    () = msg![env; button addTarget:delegate
                             action:(env.objc.lookup_selector(toggle_selector).unwrap())
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; group_view addSubview:button];

    // The dropdown list, placed on the Settings screen (not inside the
    // scroll view, so it is never clipped). Clipped to its own bounds.
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
    let menu_frame = CGRect {
        origin: menu_origin,
        size: CGSize {
            width: SETTINGS_DROPDOWN_MENU_WIDTH,
            height: visible_menu_height,
        },
    };
    let menu_view: id = msg_class![env; UIView alloc];
    let menu_view: id = msg![env; menu_view initWithFrame:menu_frame];
    () = msg![env; menu_view setBackgroundColor:dark_gray];
    () = msg![env; menu_view setClipsToBounds:true];
    () = msg![env; menu_view setHidden:true];
    () = msg![env; root_view addSubview:menu_view];

    let scroll_view: id = msg_class![env; UIScrollView alloc];
    let scroll_view_frame = rect(0.0, 0.0, list_width, visible_menu_height);
    let scroll_view: id = msg![env; scroll_view initWithFrame:scroll_view_frame];
    () = msg![env; scroll_view setClipsToBounds:true];
    () = msg![env; scroll_view setShowsVerticalScrollIndicator:false];
    () = msg![env; scroll_view setScrollEnabled:true];
    let content_size = CGSize {
        width: list_width,
        height: entries.len() as CGFloat * DEVICE_MENU_ITEM_HEIGHT,
    };
    () = msg![env; scroll_view setContentSize:content_size];
    () = msg![env; menu_view addSubview:scroll_view];

    // List items stay in content coordinates; UIScrollView moves them during
    // finger drags. Offscreen rows are also hidden for layer clipping.
    let item_selector = env.objc.lookup_selector(item_selector).unwrap();
    let white: id = msg_class![env; UIColor whiteColor];
    let mut items: Vec<id> = Vec::new();
    for (j, (title, tag)) in entries.into_iter().enumerate() {
        let y_pos = (j as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
        let item_frame = CGRect {
            origin: CGPoint { x: 0.0, y: y_pos },
            size: CGSize {
                width: list_width,
                height: DEVICE_MENU_ITEM_HEIGHT,
            },
        };
        let item_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::from_rust_string(env, title);
        () = msg![env; item_btn setTitle:text forState:UIControlStateNormal];
        release(env, text);
        () = msg![env; item_btn setTitleColor:white forState:UIControlStateNormal];
        () = msg![env; item_btn setFrame:item_frame];
        () = msg![env; item_btn layoutSubviews];
        () = msg![env; item_btn setTag:tag];
        () = msg![env; item_btn addTarget:delegate
                                   action:item_selector
                         forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; scroll_view addSubview:item_btn];
        items.push(item_btn);
    }

    // Scrollbar track (full height) and thumb.
    let track_view: id = msg_class![env; UIView alloc];
    let track_frame = CGRect {
        origin: CGPoint {
            x: list_width,
            y: 0.0,
        },
        size: CGSize {
            width: scrollbar_width,
            height: visible_menu_height,
        },
    };
    let track_view: id = msg![env; track_view initWithFrame:track_frame];
    let track_color = ui_color(env, 0.87, 0.87, 0.89, 1.0);
    () = msg![env; track_view setBackgroundColor:track_color];
    () = msg![env; menu_view addSubview:track_view];

    let thumb_view: id = msg_class![env; UIView alloc];
    let thumb_frame = CGRect {
        origin: CGPoint {
            x: list_width,
            y: 0.0,
        },
        size: CGSize {
            width: scrollbar_width,
            height: 54.0,
        },
    };
    let thumb_view: id = msg![env; thumb_view initWithFrame:thumb_frame];
    let thumb_color = ui_color(env, 0.55, 0.55, 0.58, 1.0);
    () = msg![env; thumb_view setBackgroundColor:thumb_color];
    () = msg![env; menu_view addSubview:thumb_view];

    // A single control supports clicking the track and dragging the thumb.
    let clear: id = msg_class![env; UIColor clearColor];
    let scroll_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; scroll_btn setFrame:track_frame];
    () = msg![env; scroll_btn setBackgroundColor:clear];
    () = msg![env; scroll_btn addTarget:delegate
                               action:(env.objc.lookup_selector(scroll_selector).unwrap())
                     forControlEvents:(UIControlEventTouchDown | UIControlEventTouchDragInside | UIControlEventTouchDragExit | UIControlEventTouchDragOutside)];
    () = msg![env; menu_view addSubview:scroll_btn];

    SettingsDropdown {
        button,
        menu: menu_view,
        scroll_view,
        dimmer,
        items,
        thumb: thumb_view,
    }
}

/// Re-reads the apps directory and redraws the icon grid to match. The current
/// page is kept if it still exists. If the directory can't be read, nothing
/// changes. Returns the new number of pages, for the page dots.
fn reload_app_grid(
    env: &mut Environment,
    grid: &mut IconGridStuff,
    apps: &mut Result<Vec<AppInfo>, String>,
    current_page: &mut usize,
    apps_dir: &Path,
) -> usize {
    let mut new_apps = match enumerate_apps(apps_dir) {
        Ok(new_apps) => new_apps,
        Err(e) => {
            log!("Warning: couldn't reload the apps directory: {}", e);
            return grid.pages.len();
        }
    };
    grid.pages = compute_pages(grid.containers[0].cells.len(), new_apps.len());
    if *current_page >= grid.pages.len() {
        *current_page = grid.pages.len() - 1;
    }
    let visible = grid.visible;
    update_icon_grid(env, grid, &mut new_apps, *current_page, visible);
    *apps = Ok(new_apps);
    grid.pages.len()
}

/// Width and height of the delete dialog's box, and the height of its row of
/// buttons at the bottom.
const DELETE_DIALOG_WIDTH: CGFloat = 270.0;
const DELETE_DIALOG_HEIGHT: CGFloat = 166.0;
const DELETE_DIALOG_BUTTON_HEIGHT: CGFloat = 44.0;

/// The confirmation shown after a long press on an app icon.
struct DeleteDialog {
    /// Covers the picker. Tapping it cancels.
    dimmer: id,
    /// The alert box, with its title, message and buttons.
    panel: id,
    /// The dialog's title, set each time it opens.
    title: id,
    /// The explanatory line under the title.
    message: id,
    /// The action buttons.
    cancel: id,
    confirm: id,
    /// The separator between the two buttons.
    vertical_separator: id,
}

/// Builds one button of an alert box.
fn make_alert_button(
    env: &mut Environment,
    delegate: id,
    frame: CGRect,
    title: &str,
    color: id,
    bold: bool,
    selector: &str,
) -> id {
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; button setFrame:frame];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; button setBackgroundColor:clear];
    let text = ns_string::from_rust_string(env, title.to_string());
    () = msg![env; button setTitle:text forState:UIControlStateNormal];
    release(env, text);
    () = msg![env; button setTitleColor:color forState:UIControlStateNormal];
    // FIXME: manually calling layoutSubviews shouldn't be needed?
    () = msg![env; button layoutSubviews];
    let label: id = msg![env; button titleLabel];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:(17.0 as CGFloat)]
    } else {
        msg_class![env; UIFont systemFontOfSize:(17.0 as CGFloat)]
    };
    () = msg![env; label setFont:font];
    let sel = env.objc.lookup_selector(selector).unwrap();
    () = msg![env; button addTarget:delegate
                             action:sel
                   forControlEvents:UIControlEventTouchUpInside];
    button
}

/// Builds an alert dialog matching the old-iOS Settings screen: a white
/// grouped card with a soft gray outline, separators, and blue/red text actions.
/// [set_dialog_content] fills in the text when it opens, so the title, message
/// and confirm button are deliberately left empty here.
#[allow(clippy::too_many_arguments)]
fn make_delete_dialog(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
    cancel_selector: &str,
    confirm_selector: &str,
) -> DeleteDialog {
    let dimmer: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; dimmer setFrame:(rect(0.0, 0.0, screen_size.width, screen_size.height))];
    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.4);
    () = msg![env; dimmer setBackgroundColor:dim_color];
    let cancel_sel = env.objc.lookup_selector(cancel_selector).unwrap();
    () = msg![env; dimmer addTarget:delegate
                             action:cancel_sel
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dimmer setHidden:true];
    () = msg![env; main_view addSubview:dimmer];

    let panel_frame = rect(
        (screen_size.width - DELETE_DIALOG_WIDTH) / 2.0,
        (screen_size.height - DELETE_DIALOG_HEIGHT) / 2.0,
        DELETE_DIALOG_WIDTH,
        DELETE_DIALOG_HEIGHT,
    );
    let white: id = msg_class![env; UIColor whiteColor];
    let panel = new_view(env, panel_frame, white);
    let panel_layer: id = msg![env; panel layer];
    () = msg![env; panel_layer setCornerRadius:(12.0 as CGFloat)];
    let border = ui_color(env, 0.70, 0.72, 0.75, 1.0);
    let border_cg: id = msg![env; border CGColor];
    () = msg![env; panel_layer setBorderColor:border_cg];
    () = msg![env; panel_layer setBorderWidth:(1.0 as CGFloat)];
    () = msg![env; panel setHidden:true];
    () = msg![env; main_view addSubview:panel];

    let black: id = msg_class![env; UIColor blackColor];
    let title = new_label(
        env,
        rect(12.0, 14.0, DELETE_DIALOG_WIDTH - 24.0, 44.0),
        "",
        17.0,
        true,
        black,
    );
    () = msg![env; title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; title setNumberOfLines:2];
    () = msg![env; panel addSubview:title];

    let message_color = ui_color(env, 0.25, 0.25, 0.25, 1.0);
    let message = new_label(
        env,
        rect(12.0, 58.0, DELETE_DIALOG_WIDTH - 24.0, 52.0),
        "",
        14.0,
        false,
        message_color,
    );
    () = msg![env; message setTextAlignment:UITextAlignmentCenter];
    () = msg![env; message setNumberOfLines:3];
    () = msg![env; panel addSubview:message];

    let separator_color = ui_color(env, 0.82, 0.83, 0.85, 1.0);
    let button_y = DELETE_DIALOG_HEIGHT - DELETE_DIALOG_BUTTON_HEIGHT;
    let half = DELETE_DIALOG_WIDTH / 2.0;
    let separator = new_view(
        env,
        rect(0.0, button_y, DELETE_DIALOG_WIDTH, 1.0),
        separator_color,
    );
    () = msg![env; panel addSubview:separator];
    let v_separator = new_view(
        env,
        rect(half, button_y, 1.0, DELETE_DIALOG_BUTTON_HEIGHT),
        separator_color,
    );
    () = msg![env; panel addSubview:v_separator];

    let blue = ui_color(env, 0.0, 0.478, 1.0, 1.0);
    let red = ui_color(env, 0.88, 0.18, 0.20, 1.0);
    let cancel = make_alert_button(
        env,
        delegate,
        rect(0.0, button_y, half, DELETE_DIALOG_BUTTON_HEIGHT),
        "Cancel",
        blue,
        false,
        cancel_selector,
    );
    () = msg![env; panel addSubview:cancel];
    let confirm = make_alert_button(
        env,
        delegate,
        rect(half, button_y, half, DELETE_DIALOG_BUTTON_HEIGHT),
        "Action",
        red,
        true,
        confirm_selector,
    );
    () = msg![env; panel addSubview:confirm];

    DeleteDialog {
        dimmer,
        panel,
        title,
        message,
        cancel,
        confirm,
        vertical_separator: v_separator,
    }
}

/// How long the delete dialog takes to appear, and the scale its box starts
/// from (it grows to full size while it fades in).
const DELETE_DIALOG_APPEAR_DURATION: Duration = Duration::from_millis(200);
const DELETE_DIALOG_START_SCALE: f64 = 0.8;

/// Opens `dialog` with the old iOS alert pop-in: the screen dims and the box
/// grows and fades in. `title`, `message` and the buttons must be set first
/// (see [set_dialog_content]).
fn show_delete_dialog(env: &mut Environment, run_loop: id, delegate: id, dialog: &DeleteDialog) {
    () = msg![env; (dialog.dimmer) setAlpha:(0.0 as CGFloat)];
    () = msg![env; (dialog.dimmer) setHidden:false];
    () = msg![env; (dialog.panel) setAlpha:(0.0 as CGFloat)];
    () = msg![env; (dialog.panel) setHidden:false];

    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_APPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, true);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; (dialog.panel) setTransform:transform];
            () = msg![env; (dialog.panel) setAlpha:alpha];
            () = msg![env; (dialog.dimmer) setAlpha:alpha];
        },
    );
}

/// Fills in a dialog's text and buttons. `confirm` is the title of the red
/// action button, or `None` for a single-button notice.
fn set_dialog_content(
    env: &mut Environment,
    dialog: &DeleteDialog,
    title: &str,
    message: &str,
    confirm: Option<&str>,
) {
    let title_text = ns_string::from_rust_string(env, title.to_string());
    () = msg![env; (dialog.title) setText:title_text];
    release(env, title_text);
    let message_text = ns_string::from_rust_string(env, message.to_string());
    () = msg![env; (dialog.message) setText:message_text];
    release(env, message_text);
    let cancel_width = match confirm {
        Some(confirm) => {
            let text = ns_string::from_rust_string(env, confirm.to_string());
            () = msg![env; (dialog.confirm) setTitle:text forState:UIControlStateNormal];
            release(env, text);
            () = msg![env; (dialog.confirm) setHidden:false];
            () = msg![env; (dialog.vertical_separator) setHidden:false];
            DELETE_DIALOG_WIDTH / 2.0
        }
        None => {
            () = msg![env; (dialog.confirm) setHidden:true];
            () = msg![env; (dialog.vertical_separator) setHidden:true];
            DELETE_DIALOG_WIDTH
        }
    };
    let frame: CGRect = msg![env; (dialog.cancel) frame];
    () = msg![env; (dialog.cancel) setFrame:(CGRect {
        origin: frame.origin,
        size: CGSize {
            width: cancel_width,
            height: frame.size.height,
        },
    })];
}

/// How long the delete dialog takes to disappear. It is the reverse of
/// [show_delete_dialog]: the box shrinks and fades out, and the screen clears.
const DELETE_DIALOG_DISAPPEAR_DURATION: Duration = Duration::from_millis(150);

/// Closes the delete dialog with the reverse of its pop-in animation. This
/// returns once the dialog is hidden.
fn hide_delete_dialog(env: &mut Environment, run_loop: id, delegate: id, dialog: &DeleteDialog) {
    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_DISAPPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, false);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; (dialog.panel) setTransform:transform];
            () = msg![env; (dialog.panel) setAlpha:alpha];
            () = msg![env; (dialog.dimmer) setAlpha:alpha];
        },
    );
    () = msg![env; (dialog.dimmer) setHidden:true];
    () = msg![env; (dialog.panel) setHidden:true];
}

/// Whether `dialog` is currently on screen. Used to pause saving while a
/// dialog is up, so only the dialog's own buttons change the settings.
fn is_dialog_visible(env: &mut Environment, dialog: &DeleteDialog) -> bool {
    !msg![env; (dialog.panel) isHidden]
}

/// The value a settings switch named `selector` must show for `settings`.
/// Returns `None` for a name that no switch uses.
fn toggle_value_for_selector(settings: &PickerSettings, selector: &str) -> Option<bool> {
    Some(match selector {
        "cheatEngine:" => settings.cheat_engine,
        "network:" => settings.network,
        "gameCenterAuthenticated:" => settings.game_center_authenticated,
        "analogStickTiltControls:" => settings.analog_stick_tilt_controls,
        "fullscreen:" => settings.fullscreen,
        "preserveAspectRatio:" => settings.preserve_aspect_ratio,
        "forceComposition:" => settings.force_composition,
        "showFPS:" => settings.show_fps,
        "traceGLErrors:" => settings.trace_gl_errors,
        "traceGLDraws:" => settings.trace_gl_draws,
        "printFPS:" => settings.print_fps,
        "popupErrors:" => settings.popup_errors,
        "rotationFix:" => settings.rotation_fix.unwrap_or(false),
        "moviePlayerFix:" => settings.fix_movie_player.unwrap_or(false),
        "showLockScreen:" => settings.show_lock_screen,
        _ => return None,
    })
}

/// Puts every control on the Settings screen back in step with `settings`.
/// Needed after a reset, which replaces all the settings at once. The caller
/// refreshes the device and language dropdowns, because it owns their scroll
/// state.
fn refresh_settings_controls(
    env: &mut Environment,
    settings: &SettingsStuff,
    picker_settings: &PickerSettings,
) {
    update_scale_hack_buttons(
        env,
        &settings.scale_hack_buttons,
        picker_settings.scale_hack,
    );
    update_orientation_buttons(
        env,
        &settings.orientation_buttons,
        picker_settings.orientation,
    );
    update_performance_overlay_buttons(
        env,
        &settings.overlay_mode_buttons,
        &settings.overlay_corner_buttons,
        picker_settings.overlay_mode,
        picker_settings.overlay_corner,
    );
    update_segment_buttons(
        env,
        &settings.corruption_mode_buttons,
        picker_settings.corruption_mode.selected_index() as usize,
    );
    update_segment_buttons(
        env,
        &settings.corruption_startup_delay_buttons,
        picker_settings.corruption_startup_delay.selected_index(),
    );
    update_segment_buttons(
        env,
        &settings.corruption_cooldown_buttons,
        picker_settings.corruption_cooldown.selected_index(),
    );
    update_segment_buttons(
        env,
        &settings.gles2_mode_buttons,
        picker_settings.gles2_mode.selected_index() as usize,
    );
    update_segment_buttons(
        env,
        &settings.vsync_buttons,
        vsync_selected_index(picker_settings.vsync),
    );
    update_segment_buttons(
        env,
        &settings.present_mode_buttons,
        present_mode_selected_index(picker_settings.present_mode),
    );
    update_segment_buttons(
        env,
        &settings.texture_filter_buttons,
        texture_filter_selected_index(picker_settings.fix_texture_min_filter),
    );
    update_segment_buttons(
        env,
        &settings.present_rotate_buttons,
        present_rotate_selected_index(picker_settings.present_rotate),
    );
    update_segment_buttons(
        env,
        &settings.controller_to_touch_buttons,
        controller_to_touch_selected_index(picker_settings.controller_to_touch),
    );
    for &(selector, switch) in &settings.toggle_switches {
        if let Some(value) = toggle_value_for_selector(picker_settings, selector) {
            () = msg![env; switch setOn:value];
        }
    }
    let (width, height) = picker_touch_screen_size(&env.options, picker_settings);
    refresh_manual_value_fields(
        env,
        &settings.manual_value_fields,
        width,
        height,
        &mut picker_settings.clone(),
    );
    refresh_dropdown(
        env,
        &settings.renderer_dropdown,
        &renderer_mode_label_for_tag(Some(renderer_mode_tag(picker_settings.renderer_mode))),
        &DropdownState {
            selected: Some(renderer_mode_tag(picker_settings.renderer_mode)),
            open: false,
            scroll: 0,
        },
    );
}

/// Whether the picker can offer a choice of renderer. The reset restores the
/// default that `PickerSettings::load` starts from, so it needs the same flag.
fn renderer_mode_dropdown_enabled() -> bool {
    cfg!(target_os = "android") && crate::window::angle_backend_available()
}
/// The settings the reset handler may change. Bundled so that the handler
/// takes few arguments; the caller copies them in and back out.
struct SettingsDrafts {
    picker_settings: PickerSettings,
    last_saved_picker_settings: PickerSettings,
    device_state: DropdownState,
    language_state: DropdownState,
    renderer_state: DropdownState,
}
/// Applies the reset settings screen request now that `env` and the
/// delegate's host object are no longer borrowed at the same time.
fn handle_reset_settings_request(
    env: &mut Environment,
    request: ResetSettingsRequest,
    main_run_loop: id,
    delegate: id,
    screen: (
        &SettingsStuff,
        &DeleteDialog,
        &DeleteDialog,
        &mut SettingsDrafts,
    ),
) {
    let (settings, reset_dialog, save_error_dialog, drafts) = screen;
    let picker_settings = &mut drafts.picker_settings;
    let last_saved_picker_settings = &mut drafts.last_saved_picker_settings;
    let device_state = &mut drafts.device_state;
    let language_state = &mut drafts.language_state;
    let renderer_state = &mut drafts.renderer_state;
    match request {
        ResetSettingsRequest::None => {}
        ResetSettingsRequest::Ask => {
            set_dialog_content(
                env,
                reset_dialog,
                "Reset all settings?",
                "This resets every setting to its default. Apps and their saved data are kept.",
                Some("Reset"),
            );
            show_delete_dialog(env, main_run_loop, delegate, reset_dialog);
        }
        ResetSettingsRequest::Confirm => {
            hide_delete_dialog(env, main_run_loop, delegate, reset_dialog);
            let defaults = PickerSettings::factory_defaults(renderer_mode_dropdown_enabled());
            // Persist first: if the write fails, the live settings and the
            // controls must keep showing what is actually on disk.
            match defaults.save() {
                Ok(()) => {
                    *picker_settings = defaults;
                    *last_saved_picker_settings = picker_settings.clone();
                    refresh_settings_controls(env, settings, picker_settings);
                    device_state.selected = picker_settings.device_model_tag;
                    language_state.selected = picker_settings.language_tag;
                    renderer_state.selected =
                        Some(renderer_mode_tag(picker_settings.renderer_mode));
                    refresh_dropdown(
                        env,
                        &settings.renderer_dropdown,
                        &renderer_mode_label_for_tag(renderer_state.selected),
                        renderer_state,
                    );
                    refresh_dropdown(
                        env,
                        &settings.device_dropdown,
                        &device_model_label_for_tag(device_state.selected),
                        device_state,
                    );
                    refresh_dropdown(
                        env,
                        &settings.language_dropdown,
                        &language_label_for_tag(language_state.selected),
                        language_state,
                    );
                }
                Err(error) => {
                    log!("Warning: couldn't save app picker settings: {error}");
                    set_dialog_content(
                        env,
                        save_error_dialog,
                        "Settings not saved",
                        "The settings couldn't be saved to disk, so they were kept as they were.",
                        None,
                    );
                    show_delete_dialog(env, main_run_loop, delegate, save_error_dialog);
                }
            }
        }
        ResetSettingsRequest::DismissError => {
            hide_delete_dialog(env, main_run_loop, delegate, save_error_dialog);
        }
    }
}

/// What the settings screen asked for this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResetSettingsRequest {
    None,
    Ask,
    Confirm,
    DismissError,
}

fn quick_options_trainer_enabled(options: &Options) -> bool {
    !options.trainer_disabled
}

fn quick_options_trainer_argument(enabled: bool) -> &'static str {
    if enabled {
        "--trainer"
    } else {
        "--no-trainer"
    }
}

fn quick_options_trace_gl_errors_argument(enabled: bool) -> &'static str {
    if enabled {
        "--trace-gl-errors"
    } else {
        "--no-trace-gl-errors"
    }
}

fn quick_options_trace_gl_draws_argument(enabled: bool) -> &'static str {
    if enabled {
        "--trace-gl-draws"
    } else {
        "--no-trace-gl-draws"
    }
}

fn quick_options_force_composition_argument(enabled: bool) -> &'static str {
    if enabled {
        "--force-composition"
    } else {
        "--no-force-composition"
    }
}

/// Returns the effective renderer mode and whether it can be changed in the picker.
fn quick_options_renderer_mode_state(
    renderer_mode: RendererMode,
    angle_available: bool,
) -> (RendererMode, bool) {
    let mode = if renderer_mode == RendererMode::Angle && !angle_available {
        RendererMode::NativeGles
    } else {
        renderer_mode
    };
    (mode, angle_available)
}

fn quick_options_rotation_fix_argument(enabled: bool) -> &'static str {
    if enabled {
        "--rotation-fix"
    } else {
        "--no-rotation-fix"
    }
}

fn quick_options_fix_movie_player_argument(enabled: bool) -> &'static str {
    if enabled {
        "--fix-movie-player"
    } else {
        "--no-fix-movie-player"
    }
}

/// Launch arguments for the two compatibility fix switches. An untouched
/// switch (`None`) emits nothing: `--no-rotation-fix` / `--no-fix-movie-player`
/// are explicit overrides that would cancel the app-specific defaults in
/// touchHLE_default_options.txt (GTA III needs both fixes).
fn quick_options_fix_arguments(
    rotation_fix: Option<bool>,
    fix_movie_player: Option<bool>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(enabled) = rotation_fix {
        args.push(quick_options_rotation_fix_argument(enabled).to_string());
    }
    if let Some(enabled) = fix_movie_player {
        args.push(quick_options_fix_movie_player_argument(enabled).to_string());
    }
    args
}

fn quick_options_picker_settings_arguments(settings: &PickerSettings) -> Vec<String> {
    let mut args = vec![
        format!("--vsync={}", vsync_short_name(settings.vsync)),
        format!(
            "--present-mode={}",
            present_mode_short_name(settings.present_mode)
        ),
        if settings.print_fps {
            "--print-fps".to_string()
        } else {
            "--no-print-fps".to_string()
        },
        if settings.popup_errors {
            "--error-popup".to_string()
        } else {
            "--no-error-popup".to_string()
        },
    ];
    for (tag, name, value) in [
        (MANUAL_DEADZONE, "deadzone", settings.deadzone),
        (MANUAL_X_TILT_RANGE, "x-tilt-range", settings.x_tilt_range),
        (MANUAL_Y_TILT_RANGE, "y-tilt-range", settings.y_tilt_range),
        (
            MANUAL_X_TILT_OFFSET,
            "x-tilt-offset",
            settings.x_tilt_offset,
        ),
        (
            MANUAL_Y_TILT_OFFSET,
            "y-tilt-offset",
            settings.y_tilt_offset,
        ),
    ] {
        if settings.manual_value_overrides.contains(&tag) {
            args.push(format!("--{name}={value}"));
        }
    }
    if settings.manual_value_overrides.contains(&MANUAL_FPS_LIMIT) {
        args.push(format!(
            "--fps-limit={}",
            format_optional_float(settings.fps_limit)
        ));
    }
    if let Some(enabled) = settings.fix_texture_min_filter {
        args.push(if enabled {
            "--fix-texture-min-filter".to_string()
        } else {
            "--no-fix-texture-min-filter".to_string()
        });
    }
    if let Some(degrees) = settings.present_rotate {
        args.push(format!("--present-rotate={degrees}"));
    }
    if let Some(enabled) = settings.controller_to_touch {
        args.push("--no-controller-to-touch".to_string());
        if enabled {
            for button in CONTROLLER_BUTTONS {
                if let Some((x, y)) = settings.button_to_touch.get(&button) {
                    args.push(format!("--button-to-touch={},{x},{y}", button_name(button)));
                }
            }
            if let Some((x, y, width, height)) = settings.dpad_to_touch {
                args.push(format!("--dpad-to-touch={x},{y},{width},{height}"));
            }
            if let Some((x, y, width, height)) = settings.stick_to_touch {
                args.push(format!("--stick-to-touch={x},{y},{width},{height}"));
            }
        }
    }
    args.extend(quick_options_corruption_arguments(
        settings.corruption_mode,
        settings.corruption_startup_delay,
        settings.corruption_cooldown,
        settings.corruption_bytes_per_burst,
        settings.corruption_max_offset,
        settings.corruption_seed,
    ));
    args
}

fn quick_options_corruption_arguments(
    mode: CorruptionMode,
    startup_delay: StartupDelayPreset,
    cooldown: CooldownPreset,
    bytes_per_burst: u32,
    max_offset: Option<u32>,
    seed: u64,
) -> Vec<String> {
    if mode == CorruptionMode::Off {
        return vec!["--no-corrupt-game".to_string()];
    }
    let mut args = vec!["--corrupt-game".to_string()];
    args.push(format!("--corrupt-intensity={bytes_per_burst}"));
    args.push(format!(
        "--corrupt-start-delay={}",
        startup_delay.duration().as_secs_f64()
    ));
    args.push(format!(
        "--corrupt-cooldown={}",
        cooldown.duration().as_secs_f64()
    ));
    args.push(format!("--corrupt-seed={seed}"));
    args.push(max_offset.map_or_else(
        || "--no-corrupt-max-offset".to_string(),
        |offset| format!("--corrupt-max-offset={offset}"),
    ));
    args
}

/// Explicit launch argument so the picker choice overrides option-file values.
fn quick_options_renderer_mode_argument(mode: RendererMode) -> &'static str {
    match mode {
        RendererMode::Angle => "--renderer-mode=angle",
        RendererMode::NativeGles => "--renderer-mode=native-gles",
    }
}

#[cfg(test)]
mod quick_options_trainer_tests {
    use super::*;

    #[test]
    fn trainer_toggle_defaults_off_and_emits_explicit_launch_option() {
        let mut options = Options::default();

        let mut enabled = quick_options_trainer_enabled(&options);
        assert!(!enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--no-trainer");

        options.parse_argument("--trainer").unwrap();
        enabled = quick_options_trainer_enabled(&options);
        assert!(enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--trainer");

        options.parse_argument("--no-trainer").unwrap();
        enabled = quick_options_trainer_enabled(&options);
        assert!(!enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--no-trainer");
    }
}

#[cfg(test)]
mod device_model_resolution_tests {
    use super::*;
    use crate::window::DeviceFamily;

    #[test]
    fn device_model_labels_show_portrait_pixel_resolution() {
        let entries = device_model_entries();
        for family in DeviceFamily::ALL_SELECTABLE {
            let tag = family.tag().unwrap() as NSInteger;
            let (label, _) = entries
                .iter()
                .find(|(_, entry_tag)| *entry_tag == tag)
                .unwrap();
            assert!(label.ends_with("px)"));
            assert_eq!(&device_model_label_for_tag(Some(tag as i32)), label);
        }

        for (family, expected) in [
            (DeviceFamily::iPhone3GS, "iPhone 3GS (320x480px)"),
            (DeviceFamily::iPhone4, "iPhone 4 (640x960px)"),
            (DeviceFamily::iPad3, "iPad 3 (1536x2048px)"),
        ] {
            assert_eq!(device_model_resolution_label(&family), expected);
        }
    }
}

#[cfg(test)]
mod saved_renderer_mode_tests {
    use super::*;

    #[test]
    fn migrates_legacy_values_and_prefers_current_setting() {
        assert_eq!(
            saved_renderer_mode("gles_native=true"),
            Some(RendererMode::NativeGles)
        );
        assert_eq!(
            saved_renderer_mode("gles_native=false"),
            Some(RendererMode::Angle)
        );
        assert_eq!(
            saved_renderer_mode("renderer_mode=native-gles\ngles_native=false"),
            Some(RendererMode::NativeGles)
        );
        assert_eq!(
            saved_renderer_mode("gles_native=false\nrenderer_mode=native-gles"),
            Some(RendererMode::NativeGles)
        );
    }
}

#[cfg(test)]
mod quick_options_renderer_mode_tests {
    use super::*;

    #[test]
    fn renderer_mode_dropdown_tags_round_trip() {
        let angle_tag = renderer_mode_tag(RendererMode::Angle);
        let native_tag = renderer_mode_tag(RendererMode::NativeGles);
        assert_eq!(renderer_mode_from_tag(Some(angle_tag)), RendererMode::Angle);
        assert_eq!(
            renderer_mode_from_tag(Some(native_tag)),
            RendererMode::NativeGles
        );
        assert_eq!(renderer_mode_label_for_tag(Some(angle_tag)), "ANGLE");
        assert_eq!(renderer_mode_label_for_tag(Some(native_tag)), "Native GLES");
        assert_eq!(
            renderer_mode_dropdown_spec().entries,
            vec![
                ("ANGLE".to_string(), RENDERER_MODE_ANGLE_TAG),
                ("Native GLES".to_string(), RENDERER_MODE_NATIVE_GLES_TAG),
            ]
        );
    }

    #[test]
    fn renderer_mode_follows_backend_availability_and_option_state() {
        let mut options = Options::default();

        let (mut mode, mut dropdown_enabled) =
            quick_options_renderer_mode_state(options.renderer_mode, true);
        assert_eq!(mode, RendererMode::Angle);
        assert!(dropdown_enabled);
        assert_eq!(
            quick_options_renderer_mode_argument(mode),
            "--renderer-mode=angle"
        );

        (mode, dropdown_enabled) = quick_options_renderer_mode_state(options.renderer_mode, false);
        assert_eq!(mode, RendererMode::NativeGles);
        assert!(!dropdown_enabled);
        assert_eq!(
            quick_options_renderer_mode_argument(mode),
            "--renderer-mode=native-gles"
        );

        options.parse_argument("--gles-native").unwrap();
        (mode, dropdown_enabled) = quick_options_renderer_mode_state(options.renderer_mode, true);
        assert_eq!(mode, RendererMode::NativeGles);
        assert!(dropdown_enabled);
        assert_eq!(
            quick_options_renderer_mode_argument(mode),
            "--renderer-mode=native-gles"
        );

        options.parse_argument("--no-gles-native").unwrap();
        (mode, dropdown_enabled) = quick_options_renderer_mode_state(options.renderer_mode, true);
        assert_eq!(mode, RendererMode::Angle);
        assert!(dropdown_enabled);
        assert_eq!(
            quick_options_renderer_mode_argument(mode),
            "--renderer-mode=angle"
        );
    }
}

#[cfg(test)]
mod quick_options_trace_gl_errors_tests {
    use super::*;

    #[test]
    fn switch_emits_explicit_arguments_both_ways() {
        assert_eq!(
            quick_options_trace_gl_errors_argument(true),
            "--trace-gl-errors"
        );
        assert_eq!(
            quick_options_trace_gl_errors_argument(false),
            "--no-trace-gl-errors"
        );
    }
}

#[cfg(test)]
mod quick_options_trace_gl_draws_tests {
    use super::*;

    #[test]
    fn switch_emits_explicit_arguments_both_ways() {
        assert_eq!(
            quick_options_trace_gl_draws_argument(true),
            "--trace-gl-draws"
        );
        assert_eq!(
            quick_options_trace_gl_draws_argument(false),
            "--no-trace-gl-draws"
        );
    }
}

#[cfg(test)]
mod quick_options_force_composition_tests {
    use super::*;

    #[test]
    fn switch_argument_explicitly_sets_or_clears_the_option() {
        assert_eq!(
            quick_options_force_composition_argument(true),
            "--force-composition"
        );
        assert_eq!(
            quick_options_force_composition_argument(false),
            "--no-force-composition"
        );
    }
}

#[cfg(test)]
mod quick_options_fix_tests {
    use super::*;

    #[test]
    fn switches_emit_explicit_arguments_both_ways() {
        assert_eq!(quick_options_rotation_fix_argument(true), "--rotation-fix");
        assert_eq!(
            quick_options_rotation_fix_argument(false),
            "--no-rotation-fix"
        );
        assert_eq!(
            quick_options_fix_movie_player_argument(true),
            "--fix-movie-player"
        );
        assert_eq!(
            quick_options_fix_movie_player_argument(false),
            "--no-fix-movie-player"
        );

        assert!(quick_options_fix_arguments(None, None).is_empty());
        assert_eq!(
            quick_options_fix_arguments(Some(true), None),
            vec!["--rotation-fix"]
        );
        assert_eq!(
            quick_options_fix_arguments(None, Some(false)),
            vec!["--no-fix-movie-player"]
        );
        assert_eq!(
            quick_options_fix_arguments(Some(false), Some(true)),
            vec!["--no-rotation-fix", "--fix-movie-player"]
        );
    }

    #[test]
    fn saved_gles2_and_corruption_modes_round_trip() {
        for mode in [CorruptionMode::Off, CorruptionMode::Corrupt] {
            assert_eq!(
                CorruptionMode::from_short_name(mode.short_name()),
                Some(mode)
            );
        }
        for mode in [
            Gles2Mode::Off,
            Gles2Mode::PreferContext,
            Gles2Mode::Compatibility,
        ] {
            assert_eq!(Gles2Mode::from_short_name(mode.short_name()), Some(mode));
        }
    }

    #[test]
    fn corruption_mode_arguments_override_profiles_and_include_manual_values() {
        assert_eq!(
            quick_options_corruption_arguments(
                CorruptionMode::Off,
                StartupDelayPreset::default(),
                CooldownPreset::default(),
                8,
                None,
                0,
            ),
            vec!["--no-corrupt-game"]
        );
        assert_eq!(
            quick_options_corruption_arguments(
                CorruptionMode::Corrupt,
                StartupDelayPreset::FiveSeconds,
                CooldownPreset::OneSecond,
                12,
                Some(4096),
                23,
            ),
            vec![
                "--corrupt-game",
                "--corrupt-intensity=12",
                "--corrupt-start-delay=5",
                "--corrupt-cooldown=1",
                "--corrupt-seed=23",
                "--corrupt-max-offset=4096",
            ]
        );
        assert_eq!(
            quick_options_corruption_arguments(
                CorruptionMode::Corrupt,
                StartupDelayPreset::default(),
                CooldownPreset::default(),
                8,
                None,
                0,
            ),
            vec![
                "--corrupt-game",
                "--corrupt-intensity=8",
                "--corrupt-start-delay=0",
                "--corrupt-cooldown=0.5",
                "--corrupt-seed=0",
                "--no-corrupt-max-offset",
            ]
        );
    }

    #[test]
    fn saved_overrides_round_trip_without_cancelling_cli_flags() {
        // An untouched switch is saved as "default" and must not emit an
        // argument, so app-specific defaults (e.g. GTA III) still apply.
        let mut setting = None;
        assert_eq!(optional_bool(setting), "default");
        set_bool_override(optional_bool(setting), &mut setting);
        assert_eq!(setting, None);

        set_bool_override("true", &mut setting);
        assert_eq!(setting, Some(true));
        set_bool_override("false", &mut setting);
        assert_eq!(setting, Some(false));

        // A flag passed on the command line is an explicit choice that a
        // saved "default" line must not erase.
        let mut setting = Some(true);
        set_bool_override("default", &mut setting);
        assert_eq!(setting, Some(true));

        // Unknown values must not turn an explicit override into a default
        // (or the other way round).
        set_bool_override("garbage", &mut setting);
        assert_eq!(setting, Some(true));
    }

    #[test]
    fn gles2_modes_emit_their_distinct_arguments() {
        assert!(Gles2Mode::Off.arguments().is_empty());
        assert_eq!(
            Gles2Mode::PreferContext.arguments(),
            ["--prefer-gles2-context"]
        );
        assert_eq!(Gles2Mode::Compatibility.arguments(), ["--gles2-compat"]);
    }

    #[test]
    fn compatibility_mode_does_not_emit_prefer_context() {
        let args = Gles2Mode::Compatibility.arguments();
        assert!(args.contains(&"--gles2-compat".to_string()));
        assert!(!args.contains(&"--prefer-gles2-context".to_string()));
    }
}

#[cfg(test)]
mod manual_settings_tests {
    use super::*;

    #[test]
    fn atomic_settings_write_keeps_other_data_and_cleans_up_after_failure() {
        let dir = std::env::temp_dir().join(format!("picker-write-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("settings.txt");
        let app_data = dir.join("app-data.txt");
        std::fs::write(&settings_path, "old").unwrap();
        std::fs::write(&app_data, "saved game").unwrap();

        atomic_write_picker_settings(&settings_path, b"new").unwrap();
        assert_eq!(std::fs::read(&settings_path).unwrap(), b"new");
        assert!(atomic_write_picker_settings(&dir, b"failure").is_err());
        assert_eq!(std::fs::read(&settings_path).unwrap(), b"new");
        assert_eq!(std::fs::read(&app_data).unwrap(), b"saved game");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

        std::fs::remove_file(settings_path).unwrap();
        std::fs::remove_file(app_data).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    fn settings() -> PickerSettings {
        PickerSettings::from_options(&Options::default(), true)
    }

    #[test]
    fn manual_values_clamp_and_invalid_text_preserves_previous_value() {
        let mut settings = settings();
        let oversized_integer = "9".repeat(200);
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_DEADZONE, "2", 320.0, 480.0),
            Some("0.95".to_string())
        );
        assert_eq!(settings.deadzone, 0.95);
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_DEADZONE, "not a number", 320.0, 480.0),
            None
        );
        assert_eq!(settings.deadzone, 0.95);
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_DEADZONE, "App default", 320.0, 480.0),
            None
        );
        assert_eq!(settings.deadzone, 0.95);
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_FPS_LIMIT, "999", 320.0, 480.0),
            Some("240".to_string())
        );
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_FPS_LIMIT, "1e1000", 320.0, 480.0),
            Some("240".to_string())
        );
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_FPS_LIMIT, "Unlimited", 320.0, 480.0),
            Some("Unlimited".to_string())
        );
        assert_eq!(
            apply_manual_value(
                &mut settings,
                MANUAL_CORRUPTION_MAX_OFFSET,
                "unbounded",
                320.0,
                480.0
            ),
            Some("Unbounded".to_string())
        );
        assert_eq!(
            apply_manual_value(
                &mut settings,
                MANUAL_CORRUPTION_INTENSITY,
                &oversized_integer,
                320.0,
                480.0,
            ),
            Some("64".to_string())
        );
        assert_eq!(
            apply_manual_value(
                &mut settings,
                MANUAL_CORRUPTION_SEED,
                &oversized_integer,
                320.0,
                480.0,
            ),
            Some(u64::MAX.to_string())
        );
        assert_eq!(
            apply_manual_value(&mut settings, MANUAL_CORRUPTION_SEED, "-1", 320.0, 480.0),
            Some("0".to_string())
        );
    }

    #[test]
    fn touch_manual_values_stay_within_current_screen() {
        let mut settings = settings();
        let button_tag = manual_button_tag(Button::A);
        assert_eq!(
            apply_manual_value(&mut settings, button_tag, "999,-1", 320.0, 480.0),
            Some("320,0".to_string())
        );
        assert_eq!(settings.button_to_touch[&Button::A], (320.0, 0.0));
        assert_eq!(settings.controller_to_touch, Some(true));
        assert!(settings.manual_value_overrides.contains(&button_tag));
        assert_eq!(
            apply_manual_value(&mut settings, button_tag, "App default", 320.0, 480.0,),
            Some("App default".to_string())
        );
        assert_eq!(settings.controller_to_touch, None);
        assert!(!settings.manual_value_overrides.contains(&button_tag));
        assert_eq!(
            apply_manual_value(
                &mut settings,
                MANUAL_DPAD_TOUCH,
                "300,470,50,50",
                320.0,
                480.0
            ),
            Some("300,470,20,10".to_string())
        );
        assert_eq!(settings.dpad_to_touch, Some((300.0, 470.0, 20.0, 10.0)));
    }

    #[test]
    fn controller_deadzone_enablement_tracks_stick_mapping_not_tilt() {
        let mut settings = settings();
        settings.analog_stick_tilt_controls = false;
        settings.controller_to_touch = Some(true);
        settings.stick_to_touch = Some((0.0, 0.0, 100.0, 100.0));
        assert!(manual_value_enabled(MANUAL_DEADZONE, &settings));
        settings.stick_to_touch = None;
        assert!(!manual_value_enabled(MANUAL_DEADZONE, &settings));
        assert!(!manual_value_enabled(MANUAL_X_TILT_RANGE, &settings));
        settings.controller_to_touch = None;
        assert!(manual_value_enabled(MANUAL_DEADZONE, &settings));
    }

    #[test]
    fn manual_override_persistence_ignores_unknown_tags() {
        let saved = format!("{MANUAL_DEADZONE},999999,{MANUAL_X_TILT_OFFSET}");
        assert_eq!(
            parse_manual_value_overrides(&saved),
            BTreeSet::from([MANUAL_DEADZONE, MANUAL_X_TILT_OFFSET])
        );
    }

    #[test]
    fn reset_restores_factory_defaults_not_the_command_line() {
        // "Reset all settings" must land on the same baseline as a first
        // launch, whatever the command line said when this picker started.
        let defaults = PickerSettings::factory_defaults(false);
        assert_eq!(defaults.fps_limit, Some(60.0));
        assert_eq!(defaults.scale_hack, None);
        assert_eq!(defaults.orientation, None);
        assert!(!defaults.fullscreen);
        assert_eq!(defaults.renderer_mode, RendererMode::NativeGles);
    }

    #[test]
    fn picker_settings_emit_explicit_values_and_preserve_app_defaults() {
        let mut settings = settings();
        settings.fix_texture_min_filter = None;
        settings.present_rotate = None;
        let default_args = quick_options_picker_settings_arguments(&settings);
        assert!(!default_args
            .iter()
            .any(|arg| arg.starts_with("--present-rotate=")));
        assert!(!default_args
            .iter()
            .any(|arg| arg.contains("fix-texture-min-filter")));
        assert!(!default_args.contains(&"--no-controller-to-touch".to_string()));
        assert!(default_args.contains(&"--no-corrupt-game".to_string()));
        assert!(!default_args
            .iter()
            .any(|arg| arg.starts_with("--deadzone=")));
        assert!(!default_args
            .iter()
            .any(|arg| arg.starts_with("--x-tilt-offset=")));
        assert!(!default_args
            .iter()
            .any(|arg| arg.starts_with("--fps-limit=")));
        assert_eq!(
            manual_value_text(&settings, MANUAL_X_TILT_OFFSET).as_deref(),
            Some("App default")
        );
        assert_eq!(
            manual_value_text(&settings, manual_button_tag(Button::A)).as_deref(),
            Some("App default")
        );

        settings.vsync = VsyncMode::On;
        settings.present_mode = PresentMode::Readback;
        settings.fix_texture_min_filter = Some(false);
        settings.present_rotate = Some(90);
        settings.deadzone = 0.25;
        settings.x_tilt_range = 120.0;
        settings.fps_limit = Some(120.0);
        settings.print_fps = true;
        settings.popup_errors = false;
        settings.controller_to_touch = Some(true);
        settings.button_to_touch.insert(Button::A, (12.0, 34.0));
        settings.dpad_to_touch = Some((1.0, 2.0, 3.0, 4.0));
        settings.manual_value_overrides.extend([
            MANUAL_DEADZONE,
            MANUAL_X_TILT_RANGE,
            MANUAL_FPS_LIMIT,
        ]);
        let args = quick_options_picker_settings_arguments(&settings);
        for expected in [
            "--vsync=on",
            "--present-mode=readback",
            "--no-fix-texture-min-filter",
            "--present-rotate=90",
            "--deadzone=0.25",
            "--x-tilt-range=120",
            "--fps-limit=120",
            "--print-fps",
            "--no-error-popup",
            "--button-to-touch=A,12,34",
            "--dpad-to-touch=1,2,3,4",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "missing {expected}");
        }
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == "--no-controller-to-touch")
                .count(),
            1
        );
        settings.controller_to_touch = None;
        let app_default_args = quick_options_picker_settings_arguments(&settings);
        assert!(!app_default_args.contains(&"--no-controller-to-touch".to_string()));
        assert!(!app_default_args
            .iter()
            .any(|arg| arg.starts_with("--button-to-touch=")));
        assert_eq!(
            apply_manual_value(
                &mut settings,
                MANUAL_X_TILT_OFFSET,
                "App default",
                320.0,
                480.0
            ),
            Some("App default".to_string())
        );
        assert!(!settings
            .manual_value_overrides
            .contains(&MANUAL_X_TILT_OFFSET));
    }
}

#[cfg(test)]
mod unlock_animation_tests {
    use super::*;

    #[test]
    fn ios4_unlock_flies_in_radially_from_off_screen_to_grid_slots() {
        let screen = CGSize {
            width: 320.0,
            height: 460.0,
        };
        let mut centers = Vec::new();
        let mut label_offsets = Vec::new();
        for row in 0..4 {
            for col in 0..3 {
                centers.push(CGPoint {
                    x: 69.0 + (col as CGFloat) * 91.0,
                    y: 80.0 + (row as CGFloat) * 88.0,
                });
                label_offsets.push(44.0);
            }
        }
        let targets = compute_unlock_cell_targets(&centers, &label_offsets, screen);
        assert_eq!(targets.len(), 12);

        // Every cell starts completely off-screen at t = 0.0 and lands at its
        // resting slot with identity scale and full opacity at t = 1.0.
        for (center, target) in centers.iter().zip(targets.iter()) {
            let (dx0, dy0, _, scale0, alpha0) = unlock_cell_state_at(target, 0.0);
            let start_x = center.x + dx0;
            let start_y = center.y + dy0;
            let off_screen = start_x < -ICON_SIZE.width / 2.0
                || start_x > screen.width + ICON_SIZE.width / 2.0
                || start_y < -ICON_SIZE.height / 2.0
                || start_y > screen.height + ICON_SIZE.height / 2.0;
            assert!(off_screen);
            assert!((scale0 - HOME_UNLOCK_START_SCALE).abs() < 1e-4);
            assert_eq!(alpha0, 0.0);

            let (dx1, dy1, label_dy1, scale1, alpha1) = unlock_cell_state_at(target, 1.0);
            assert!(dx1.abs() < 1e-4);
            assert!(dy1.abs() < 1e-4);
            assert!(label_dy1.abs() < 1e-4);
            assert!((scale1 - 1.0).abs() < 1e-4);
            assert!((alpha1 - 1.0).abs() < 1e-4);
        }

        // Corner cells are staggered behind inner cells.
        assert!(targets[0].norm_dist > targets[4].norm_dist);
    }
}

#[cfg(test)]
mod pop_dialog_animation_tests {
    use super::*;

    #[test]
    fn settings_and_delete_share_centered_pop_profile_and_timing() {
        assert_eq!(DELETE_DIALOG_APPEAR_DURATION, Duration::from_millis(200));
        assert_eq!(DELETE_DIALOG_DISAPPEAR_DURATION, Duration::from_millis(150));

        let (open_scale_0, open_alpha_0) = pop_dialog_state_at(0.0, true);
        assert!((open_scale_0 - DELETE_DIALOG_START_SCALE as CGFloat).abs() < 1e-5);
        assert_eq!(open_alpha_0, 0.0);

        let (open_scale_1, open_alpha_1) = pop_dialog_state_at(1.0, true);
        assert!((open_scale_1 - 1.0).abs() < 1e-5);
        assert!((open_alpha_1 - 1.0).abs() < 1e-5);

        let (close_scale_0, close_alpha_0) = pop_dialog_state_at(0.0, false);
        assert!((close_scale_0 - 1.0).abs() < 1e-5);
        assert!((close_alpha_0 - 1.0).abs() < 1e-5);

        let (close_scale_1, close_alpha_1) = pop_dialog_state_at(1.0, false);
        assert!((close_scale_1 - DELETE_DIALOG_START_SCALE as CGFloat).abs() < 1e-5);
        assert_eq!(close_alpha_1, 0.0);

        let (mid_open_scale, mid_open_alpha) = pop_dialog_state_at(0.5, true);
        let (mid_close_scale, mid_close_alpha) = pop_dialog_state_at(0.5, false);
        assert!(mid_open_scale > 0.95 && mid_open_alpha > 0.85);
        assert!((mid_open_alpha + mid_close_alpha - 1.0).abs() < 1e-5);
        assert!(mid_close_scale < 0.85);
    }
}
