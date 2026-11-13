/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `UIScrollView`.

pub mod ui_text_view;

use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_geometry::UIEdgeInsets;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, nil, objc_classes, ClassExports, NSZonePtr, SEL,
};

type UIScrollViewIndicatorStyle = NSInteger;
type UIScrollViewKeyboardDismissMode = NSInteger;

// UIScrollViewKeyboardDismissMode values
const UIScrollViewKeyboardDismissModeNone: NSInteger = 0;
const UIScrollViewKeyboardDismissModeOnDrag: NSInteger = 1;
const UIScrollViewKeyboardDismissModeInteractive: NSInteger = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
 Horizontal,
 Vertical,
}

pub struct UIScrollViewHostObject {
    superclass: super::UIViewHostObject,
    /// UIScrollViewDelegate — weak reference
    delegate: id,
    scroll_enabled: bool,
    content_offset: CGPoint,
    content_size: CGSize,
    content_inset: UIEdgeInsets,
    shows_horizontal_scroll_indicator: bool,
    shows_vertical_scroll_indicator: bool,
    always_bounce_vertical: bool,
    always_bounce_horizontal: bool,
    bounces: bool,
    paging_enabled: bool,
    directional_lock_enabled: bool,
 locked_axis: Option<Axis>,
 locked_cumulative: (CGFloat, CGFloat),
    minimum_zoom_scale: CGFloat,
    maximum_zoom_scale: CGFloat,
    zoom_scale: CGFloat,
    keyboard_dismiss_mode: UIScrollViewKeyboardDismissMode,
    decelerates: bool,
    scrolls_to_top: bool,             // (с прошлого фикса)
    can_cancel_content_touches: bool, // <-- ДОБАВЛЕНО
    /// `UIScrollViewIndicatorStyle` — specifies the look of the scroll
    /// indicators. Per Apple's UIScrollView reference:
    /// https://developer.apple.com/documentation/uikit/uiscrollview/1619615-indicatorstyle
    indicator_style: UIScrollViewIndicatorStyle,
}
impl_HostObject_with_superclass!(UIScrollViewHostObject);

impl Default for UIScrollViewHostObject {
    fn default() -> Self {
        UIScrollViewHostObject {
            superclass: Default::default(),
            delegate: nil,
            scroll_enabled: true,
            content_offset: CGPoint { x: 0.0, y: 0.0 },
            content_size: CGSize {
                width: 0.0,
                height: 0.0,
            },
            content_inset: UIEdgeInsets::default(),
            shows_horizontal_scroll_indicator: true,
            shows_vertical_scroll_indicator: true,
            always_bounce_vertical: false,
            always_bounce_horizontal: false,
            bounces: true,
            paging_enabled: false,
            directional_lock_enabled: false,
 locked_axis: None,
 locked_cumulative: (0.0, 0.0),
            minimum_zoom_scale: 1.0,
            maximum_zoom_scale: 1.0,
            zoom_scale: 1.0,
            keyboard_dismiss_mode: UIScrollViewKeyboardDismissModeNone,
            decelerates: true,
            scrolls_to_top: true,
            can_cancel_content_touches: true, // <-- ДОБАВЛЕНО
            indicator_style: 0,               // UIScrollViewIndicatorStyleDefault
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIScrollView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIScrollViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UIScrollViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).delegate = delegate;
}

// MARK: - Content offset

- (CGPoint)contentOffset {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_offset
}
- (())setContentOffset:(CGPoint)offset {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).content_offset = offset;

    let mut bounds: CGRect = msg![env; this bounds];
    bounds.origin = offset;
    () = msg![env; this setBounds:bounds];
    () = msg![env; this setNeedsDisplay];
}
- (())setContentOffset:(CGPoint)offset animated:(bool)_animated {
    msg![env; this setContentOffset:offset]
}

// MARK: - Content size & inset

- (CGSize)contentSize {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_size
}
- (())setContentSize:(CGSize)size {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).content_size = size;
}

// UIEdgeInsets passed as four floats (top, left, bottom, right) on the stack.
// We store them individually to avoid needing a SafeRead impl here.

- (UIEdgeInsets)contentInset {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_inset
}
- (())setContentInset:(UIEdgeInsets)inset {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).content_inset = inset;
}
- (CGFloat)contentInsetTop {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_inset.top
}
- (())setContentInset:(CGFloat)top left:(CGFloat)left bottom:(CGFloat)bottom right:(CGFloat)right {
    let host = env.objc.borrow_mut::<UIScrollViewHostObject>(this);
    host.content_inset = UIEdgeInsets { top, left, bottom, right };
}

// MARK: - Scroll enable / bounces

- (bool)scrollEnabled {
    env.objc.borrow::<UIScrollViewHostObject>(this).scroll_enabled
}
- (())setScrollEnabled:(bool)enabled {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).scroll_enabled = enabled;
}

// MARK: - Touch handling properties

- (bool)canCancelContentTouches {
    env.objc.borrow::<UIScrollViewHostObject>(this).can_cancel_content_touches
}

- (())setCanCancelContentTouches:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).can_cancel_content_touches = value;
}

// MARK: - Scroll to top

- (bool)scrollsToTop {
    env.objc.borrow::<UIScrollViewHostObject>(this).scrolls_to_top
}

- (())setScrollsToTop:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).scrolls_to_top = value;
}

- (bool)bounces {
    env.objc.borrow::<UIScrollViewHostObject>(this).bounces
}
- (())setBounces:(bool)bounces {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).bounces = bounces;
}

- (bool)alwaysBounceVertical {
    env.objc.borrow::<UIScrollViewHostObject>(this).always_bounce_vertical
}
- (())setAlwaysBounceVertical:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).always_bounce_vertical = value;
}

- (bool)alwaysBounceHorizontal {
    env.objc.borrow::<UIScrollViewHostObject>(this).always_bounce_horizontal
}
- (())setAlwaysBounceHorizontal:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).always_bounce_horizontal = value;
}

- (())setDelaysContentTouches:(bool)_value {
    // TODO
}

// MARK: - Scroll indicators

- (bool)showsHorizontalScrollIndicator {
    env.objc.borrow::<UIScrollViewHostObject>(this).shows_horizontal_scroll_indicator
}
- (())setShowsHorizontalScrollIndicator:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).shows_horizontal_scroll_indicator = value;
}

- (bool)showsVerticalScrollIndicator {
    env.objc.borrow::<UIScrollViewHostObject>(this).shows_vertical_scroll_indicator
}
- (())setShowsVerticalScrollIndicator:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).shows_vertical_scroll_indicator = value;
}

- (UIScrollViewIndicatorStyle)indicatorStyle {
    env.objc.borrow::<UIScrollViewHostObject>(this).indicator_style
}
- (())setIndicatorStyle:(UIScrollViewIndicatorStyle)style {
    // Apple defines:
    // - UIScrollViewIndicatorStyleDefault (0)
    // - UIScrollViewIndicatorStyleBlack (1)
    // - UIScrollViewIndicatorStyleWhite (2)
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).indicator_style = style;
}

- (())flashScrollIndicators {
    // No-op — we don't render scroll indicators.
}

// MARK: - Paging & locking

- (bool)isPagingEnabled {
    env.objc.borrow::<UIScrollViewHostObject>(this).paging_enabled
}
- (())setPagingEnabled:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).paging_enabled = value;
}

- (bool)isDirectionalLockEnabled {
    env.objc.borrow::<UIScrollViewHostObject>(this).directional_lock_enabled
}
- (())setDirectionalLockEnabled:(bool)value {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).directional_lock_enabled = value;
}

// MARK: - Zooming

- (CGFloat)minimumZoomScale {
    env.objc.borrow::<UIScrollViewHostObject>(this).minimum_zoom_scale
}
- (())setMinimumZoomScale:(CGFloat)scale {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).minimum_zoom_scale = scale;
}

- (CGFloat)maximumZoomScale {
    env.objc.borrow::<UIScrollViewHostObject>(this).maximum_zoom_scale
}
- (())setMaximumZoomScale:(CGFloat)scale {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).maximum_zoom_scale = scale;
}

- (CGFloat)zoomScale {
    env.objc.borrow::<UIScrollViewHostObject>(this).zoom_scale
}
- (())setZoomScale:(CGFloat)scale {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).zoom_scale = scale;
}
- (())setZoomScale:(CGFloat)scale animated:(bool)_animated {
    msg![env; this setZoomScale:scale]
}
- (())zoomToRect:(CGRect)_rect animated:(bool)_animated {
    log!("TODO: UIScrollView zoomToRect:animated:");
}

// MARK: - Keyboard dismiss

- (UIScrollViewKeyboardDismissMode)keyboardDismissMode {
    env.objc.borrow::<UIScrollViewHostObject>(this).keyboard_dismiss_mode
}
- (())setKeyboardDismissMode:(UIScrollViewKeyboardDismissMode)mode {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).keyboard_dismiss_mode = mode;
}

// MARK: - Scrolling to visible area

- (())scrollRectToVisible:(CGRect)rect animated:(bool)_animated {
    let offset: CGPoint = msg![env; this contentOffset];
    let bounds: CGRect  = msg![env; this bounds];

    let mut new_x = offset.x;
    let mut new_y = offset.y;

    // Bring rect into view horizontally.
    if rect.origin.x < offset.x {
        new_x = rect.origin.x;
    } else if rect.origin.x + rect.size.width > offset.x + bounds.size.width {
        new_x = rect.origin.x + rect.size.width - bounds.size.width;
    }

    // Bring rect into view vertically.
    if rect.origin.y < offset.y {
        new_y = rect.origin.y;
    } else if rect.origin.y + rect.size.height > offset.y + bounds.size.height {
        new_y = rect.origin.y + rect.size.height - bounds.size.height;
    }

    let new_offset = CGPoint { x: new_x, y: new_y };
    if new_offset.x != offset.x || new_offset.y != offset.y {
        msg![env; this setContentOffset:new_offset]
    }
}

// MARK: - State queries

- (bool)isDecelerating {
    false
}
- (bool)isDragging {
    false
}
- (bool)isTracking {
    false
}
- (bool)isZooming {
    false
}
- (bool)isZoomBouncing {
    false
}

// MARK: - Touch handling

- (())touchesMoved:(id)touches withEvent:(id)_event {
    let scroll_enabled: bool = msg![env; this scrollEnabled];
    if !scroll_enabled {
        return;
    }

    let touch_arr: id = msg![env; touches allObjects];
    let touch: id     = msg![env; touch_arr objectAtIndex:0u32];
    let bounds: CGRect = msg![env; this bounds];

    let prev_location: CGPoint = msg![env; touch previousLocationInView:this];
    let new_location:  CGPoint = msg![env; touch locationInView:this];

    let delta_x = new_location.x - prev_location.x;
    let delta_y = new_location.y - prev_location.y;

    let offset:       CGPoint = msg![env; this contentOffset];
    let content_size: CGSize  = msg![env; this contentSize];

    // Directional lock: with it enabled, a drag picks one axis and
 // later moves on this touch sequence only move along that axis.
 // The axis is decided by total travel, not by one noisy move.
 let locked: bool = msg![env; this isDirectionalLockEnabled];
let delta = if locked {
let axis = {
let host = env.objc.borrow_mut::<UIScrollViewHostObject>(this);
match host.locked_axis {
Some(axis) => axis,
None => {
host.locked_cumulative = (
host.locked_cumulative.0 + delta_x,
host.locked_cumulative.1 + delta_y,
);
let axis = lock_axis(host.locked_cumulative);
host.locked_axis = Some(axis);
axis
}
}
};
 match axis {
 Axis::Horizontal => (delta_x, 0.0),
 Axis::Vertical => (0.0, delta_y),
 }
 } else {
 (delta_x, delta_y)
 };

 let new_offset = offset_after_drag(offset, content_size, bounds.size, delta);

    log_dbg!("UIScrollView content offset: old {:?}, new {:?}", offset, new_offset);

    if new_offset != offset {
        () = msg![env; this setContentOffset:new_offset];

        let delegate: id = msg![env; this delegate];
        if delegate != nil {
            let sel: SEL = env.objc.register_host_selector(
                "scrollViewDidScroll:".to_string(),
                &mut env.mem,
            );

            let responds: bool = msg![env; delegate respondsToSelector:sel];
            if responds {
                () = msg![env; delegate scrollViewDidScroll:this];
            }
        }
    }
}

- (())touchesEnded:(id)_touches withEvent:(id)_event {
 let host = env.objc.borrow_mut::<UIScrollViewHostObject>(this);
 host.locked_axis = None;
 host.locked_cumulative = (0.0, 0.0);
    let delegate: id = msg![env; this delegate];
    if delegate != nil {
        let sel: SEL = env.objc.register_host_selector(
            "scrollViewDidEndDecelerating:".to_string(),
            &mut env.mem,
        );

        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            () = msg![env; delegate scrollViewDidEndDecelerating:this];
        }
    }
}

- (())touchesCancelled:(id)_touches withEvent:(id)_event {
 let host = env.objc.borrow_mut::<UIScrollViewHostObject>(this);
 host.locked_axis = None;
 host.locked_cumulative = (0.0, 0.0);
    let delegate: id = msg![env; this delegate];
    if delegate != nil {
        let sel: SEL = env.objc.register_host_selector(
            "scrollViewDidEndScrollingAnimation:".to_string(),
            &mut env.mem,
        );

        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            () = msg![env; delegate scrollViewDidEndScrollingAnimation:this];
        }
    }
}

@end

};
/// Which axis a drag belongs to: the one with more travel so far.
fn lock_axis(cumulative: (CGFloat, CGFloat)) -> Axis {
 if cumulative.0.abs() > cumulative.1.abs() {
 Axis::Horizontal
 } else {
 Axis::Vertical
 }
}

/// Where `offset` ends up after the finger moves by `delta`: content follows
/// the finger, and stops at its edges. A content plate no bigger than the
/// scroll view cannot move along that axis at all, so a diagonal drag on a
/// list that only scrolls up and down does not wobble it sideways.
fn offset_after_drag(
    offset: CGPoint,
    content_size: CGSize,
    bounds_size: CGSize,
    delta: (CGFloat, CGFloat),
) -> CGPoint {
    let (delta_x, delta_y) = delta;
    let max_x = (content_size.width - bounds_size.width).max(0.0);
    let max_y = (content_size.height - bounds_size.height).max(0.0);
    CGPoint {
        x: (offset.x - delta_x).clamp(0.0, max_x),
        y: (offset.y - delta_y).clamp(0.0, max_y),
    }
}

#[cfg(test)]
mod tests {
    use super::{lock_axis, offset_after_drag, Axis};
    use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGSize};

    fn size(width: CGFloat, height: CGFloat) -> CGSize {
        CGSize { width, height }
    }

    #[test]
    fn diagonal_drag_does_not_move_a_list_that_fits_sideways() {
        // The settings list: as wide as the scroll view, much taller.
        let offset = CGPoint { x: 0.0, y: 100.0 };
        let bounds = size(320.0, 480.0);
        let moved = offset_after_drag(offset, size(320.0, 2000.0), bounds, (40.0, -30.0));
        let (moved_x, moved_y) = (moved.x, moved.y);
        assert_eq!(moved_x, 0.0);
        assert_eq!(moved_y, 130.0);
    }

    #[test]
    fn drag_stops_at_the_content_edges() {
        let content = size(600.0, 2000.0);
        let bounds = size(320.0, 480.0);
        let at_bottom_right = offset_after_drag(
            CGPoint { x: 280.0, y: 1520.0 },
            content,
            bounds,
            (-100.0, -100.0),
        );
        let (at_bottom_right_x, at_bottom_right_y) = (at_bottom_right.x, at_bottom_right.y);
        assert_eq!(at_bottom_right_x, 280.0);
        assert_eq!(at_bottom_right_y, 1520.0);
        let past_top_left =
            offset_after_drag(CGPoint { x: 10.0, y: 10.0 }, content, bounds, (100.0, 100.0));
        let (past_top_left_x, past_top_left_y) = (past_top_left.x, past_top_left.y);
        assert_eq!(past_top_left_x, 0.0);
        assert_eq!(past_top_left_y, 0.0);
    }

    #[test]
    #[test]
 fn content_slightly_wider_than_the_scroll_view_can_move_sideways() {
 // The old settings layout: content 10pt wider than the scroll
 // view, so a sideways finger drag produced a 10pt x-offset.
 // Documents why content width must equal scroll view width.
 let moved = offset_after_drag(
 CGPoint { x: 0.0, y: 0.0 },
 size(330.0, 2000.0),
 size(320.0, 480.0),
 (-10.0, -100.0),
 );
 let (moved_x, moved_y) = (moved.x, moved.y);
 assert_eq!(moved_x, 10.0);
 assert_eq!(moved_y, 100.0);
 }

 #[test]
 fn lock_axis_picks_the_axis_with_more_travel() {
 assert_eq!(super::lock_axis((20.0, 5.0)), super::Axis::Horizontal);
 assert_eq!(super::lock_axis((5.0, 20.0)), super::Axis::Vertical);
 assert_eq!(super::lock_axis((10.0, 10.0)), super::Axis::Vertical);
 }

 #[test]
 fn content_smaller_than_the_scroll_view_never_moves() {
        let moved = offset_after_drag(
            CGPoint { x: 0.0, y: 0.0 },
            size(320.0, 400.0),
            size(320.0, 480.0),
            (-50.0, -50.0),
        );
        let (moved_x, moved_y) = (moved.x, moved.y);
        assert_eq!(moved_x, 0.0);
        assert_eq!(moved_y, 0.0);
    }
}
