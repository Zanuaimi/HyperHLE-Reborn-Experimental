/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITextField`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_string, NSInteger, NSRange, NSUInteger};
use crate::frameworks::uikit::ui_font::{UITextAlignment, UITextAlignmentLeft};
use crate::frameworks::uikit::ui_view::ui_window::{
    UIKeyboardDidHideNotification, UIKeyboardDidShowNotification, UIKeyboardWillHideNotification,
    UIKeyboardWillShowNotification,
};
use crate::frameworks::uikit::ui_view::ui_alert_view;
use crate::impl_HostObject_with_superclass;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, todo_objc_setter,
    ClassExports, NSZonePtr, SEL,
};
use crate::Environment;

type UIKeyboardAppearance = NSInteger;
type UIKeyboardType = NSInteger;
pub type UIReturnKeyType = NSInteger;
type UITextAutocapitalizationType = NSInteger;
type UITextAutocorrectionType = NSInteger;
type UITextBorderStyle = NSInteger;
type UITextFieldViewMode = NSInteger;

const UITextFieldTextDidChangeNotification: &str = "UITextFieldTextDidChangeNotification";
const UITextFieldTextDidBeginEditingNotification: &str =
    "UITextFieldTextDidBeginEditingNotification";
const UITextFieldTextDidEndEditingNotification: &str = "UITextFieldTextDidEndEditingNotification";

pub const CONSTANTS: ConstantExports = &[
    (
        "_UITextFieldTextDidChangeNotification",
        HostConstant::NSString(UITextFieldTextDidChangeNotification),
    ),
    (
        "_UITextFieldTextDidBeginEditingNotification",
        HostConstant::NSString(UITextFieldTextDidBeginEditingNotification),
    ),
    (
        "_UITextFieldTextDidEndEditingNotification",
        HostConstant::NSString(UITextFieldTextDidEndEditingNotification),
    ),
];

struct UITextFieldHostObject {
    superclass: super::UIControlHostObject,
    delegate: id,
    editing: bool,
    text_label: id,
    placeholder_label: id,
    background_view: id,
    placeholder: id,
    attributed_placeholder: id,
    font: id,
    text_color: id,
    text_alignment: UITextAlignment,
    border_style: UITextBorderStyle,
    background: id,
    disabled_background: id,
    clear_button_mode: UITextFieldViewMode,
    left_view: id,
    left_view_mode: UITextFieldViewMode,
    right_view: id,
    right_view_mode: UITextFieldViewMode,
    input_view: id,
    input_accessory_view: id,
    clears_on_begin_editing: bool,
    adjusts_font_size_to_fit_width: bool,
    minimum_font_size: f32,
    secure_text_entry: bool,
    autocapitalization_type: UITextAutocapitalizationType,
    autocorrection_type: UITextAutocorrectionType,
    return_key_type: UIReturnKeyType,
    keyboard_type: UIKeyboardType,
    keyboard_appearance: UIKeyboardAppearance,
    enables_return_key_automatically: bool,
    spell_checking_type: NSInteger,
    content_vertical_alignment: NSInteger,
}
impl_HostObject_with_superclass!(UITextFieldHostObject);
impl Default for UITextFieldHostObject {
    fn default() -> Self {
        UITextFieldHostObject {
            superclass: Default::default(),
            delegate: nil,
            editing: false,
            text_label: nil,
            placeholder_label: nil,
            background_view: nil,
            placeholder: nil,
            attributed_placeholder: nil,
            font: nil,
            text_color: nil,
            text_alignment: UITextAlignmentLeft,
            border_style: 0,
            background: nil,
            disabled_background: nil,
            clear_button_mode: 0,
            left_view: nil,
            left_view_mode: 0,
            right_view: nil,
            right_view_mode: 0,
            input_view: nil,
            input_accessory_view: nil,
            clears_on_begin_editing: false,
            adjusts_font_size_to_fit_width: false,
            minimum_font_size: 0.0,
            secure_text_entry: false,
            autocapitalization_type: 0,
            autocorrection_type: 0,
            return_key_type: 0,
            keyboard_type: 0,
            keyboard_appearance: 0,
            enables_return_key_automatically: false,
            spell_checking_type: 0,
            content_vertical_alignment: 0,
        }
    }
}

// Apple documents background images as content behind the editable text;
// rounded-rectangle borders ignore custom images. Upstream touchHLE leaves
// these properties unimplemented.
// https://developer.apple.com/documentation/uikit/uitextfield/borderstyle-swift.property
// https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
fn update_background(env: &mut Environment, field: id) {
    let enabled: bool = msg![env; field isEnabled];
    let host = env.objc.borrow::<UITextFieldHostObject>(field);
    let style = host.border_style;
    let image = if host.border_style == 3 { nil } else if enabled {
        host.background
    } else {
        host.disabled_background
    };
    let mut view = host.background_view;
    if view == nil && (image != nil || style != 0) {
        view = msg_class![env; UIImageView new];
        env.objc.borrow_mut::<UITextFieldHostObject>(field).background_view = view;
        () = msg![env; view setUserInteractionEnabled:false];
        () = msg![env; field insertSubview:view atIndex:0u32];
    }
    if view != nil {
        let bounds: CGRect = msg![env; field bounds];
        () = msg![env; view setFrame:bounds];
        () = msg![env; view setImage:image];
        let layer: id = msg![env; view layer];
        let bordered = style != 0 && image == nil;
        let color: id = if bordered { msg_class![env; UIColor whiteColor] }
            else { msg_class![env; UIColor clearColor] };
        () = msg![env; view setBackgroundColor:color];
        let border_color: id = msg_class![env; UIColor grayColor];
        let border_color: id = msg![env; border_color CGColor];
        () = msg![env; layer setBorderColor:border_color];
        () = msg![env; layer setBorderWidth:(if bordered { 1.0f32 } else { 0.0f32 })];
        () = msg![env; layer setCornerRadius:(if style == 3 { 6.0f32 } else { 0.0f32 })];
    }
}

fn accessory_visible(mode: UITextFieldViewMode, editing: bool) -> bool {
    mode == 3 || (mode == 1 && editing) || (mode == 2 && !editing)
}

fn field_content_rect(env: &mut Environment, field: id, mut bounds: CGRect) -> CGRect {
    let host = env.objc.borrow::<UITextFieldHostObject>(field);
    let (left, right, editing, lm, rm, border) = (host.left_view, host.right_view,
        host.editing, host.left_view_mode, host.right_view_mode, host.border_style);
    let padding = if border == 3 { 6.0 } else { 0.0 };
    let mut start = bounds.origin.x + padding;
    let mut end = bounds.origin.x + bounds.size.width - padding;
    if left != nil && accessory_visible(lm, editing) {
        let rect: CGRect = msg![env; field leftViewRectForBounds:bounds];
        start = start.max(rect.origin.x + rect.size.width);
    }
    if right != nil && accessory_visible(rm, editing) {
        let rect: CGRect = msg![env; field rightViewRectForBounds:bounds];
        end = end.min(rect.origin.x);
    }
    bounds.origin.x = start;
    bounds.size.width = (end - start).max(0.0);
    bounds
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITextField: UIControl


+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UITextFieldHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];

    // A borderless field inherits the transparent UIView background. A forced
    // white fill hides the containing bar and makes light text unreadable.
    let _: () = msg![env; this setOpaque:false];

    let text_label: id = msg_class![env; UILabel new];

    // Bind the colour first: passing `msg_class!` straight into `msg!`
    // leaves the message return type ambiguous (E0283).
    let clear_color: id = msg_class![env; UIColor clearColor];
    let _: () = msg![env; text_label setBackgroundColor:clear_color];

    let _: () = msg![env; text_label setTextAlignment:UITextAlignmentLeft];
    let text_color: id = msg_class![env; UIColor blackColor];
    let _: () = msg![env; text_label setTextColor:text_color];
    // UIKit's default text field font is the 17-point system font. It has to
    // be set eagerly: `UILabel` draws nothing at all without a font, so a
    // field whose app never sets one showed an empty box with the typed text
    // invisible inside it.
    let default_font: id = msg_class![env; UIFont systemFontOfSize:17.0];
    let _: () = msg![env; text_label setFont:default_font];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_label = text_label;
    let _: () = msg![env; this addSubview:text_label];
        // `-font` and `-textColor` are never nil on iOS, and the placeholder
        // label laid out below reuses both, so seed the stored values from the
        // same defaults the text label already uses.
        let _: () = msg![env; this setFont:default_font];
        let _: () = msg![env; this setTextColor:text_color];

    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    let text_label: id = msg_class![env; UILabel new];

    // Bind the colour first: passing `msg_class!` straight into `msg!`
    // leaves the message return type ambiguous (E0283).
    let clear_color: id = msg_class![env; UIColor clearColor];
    let _: () = msg![env; text_label setBackgroundColor:clear_color];
    let text_color: id = msg_class![env; UIColor blackColor];
    let _: () = msg![env; text_label setTextColor:text_color];
    let default_font: id = msg_class![env; UIFont systemFontOfSize:17.0];
    let _: () = msg![env; text_label setFont:default_font];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_label = text_label;
    let _: () = msg![env; this addSubview:text_label];
        // `-font` and `-textColor` are never nil on iOS, and the placeholder
        // label laid out below reuses both, so seed the stored values from the
        // same defaults the text label already uses.
        let _: () = msg![env; this setFont:default_font];
        let _: () = msg![env; this setTextColor:text_color];

    let key_text = get_static_str(env, "UIText");
    if msg![env; coder containsValueForKey:key_text] {
        let text: id = msg![env; coder decodeObjectForKey:key_text];
        () = msg![env; this setText:text];
    }

    let key_font = get_static_str(env, "UIFont");
    if msg![env; coder containsValueForKey:key_font] {
        let font: id = msg![env; coder decodeObjectForKey:key_font];
        () = msg![env; this setFont:font];
    } else {
        () = msg![env; this setFont:nil];
    }

    let key_color = get_static_str(env, "UITextColor");
    if msg![env; coder containsValueForKey:key_color] {
        let text_color: id = msg![env; coder decodeObjectForKey:key_color];
        () = msg![env; this setTextColor:text_color];
    } else {
        () = msg![env; this setTextColor:nil];
    }

    let key_align = get_static_str(env, "UITextAlignment");
    if msg![env; coder containsValueForKey:key_align] {
        let align: UITextAlignment = msg![env; coder decodeIntegerForKey:key_align];
        () = msg![env; this setTextAlignment:align];
    }

    for (key, selector) in [("UIBackground", "setBackground:"),
                            ("UIDisabledBackground", "setDisabledBackground:"),
                            ("UIPlaceholder", "setPlaceholder:")] {
        let key = get_static_str(env, key);
        if msg![env; coder containsValueForKey:key] {
            let value: id = msg![env; coder decodeObjectForKey:key];
            let selector = env.objc.register_host_selector(selector.into(), &mut env.mem);
            () = msg![env; this performSelector:selector withObject:value];
        }
    }
    for key in ["UIBorderStyle", "UIKeyboardType", "UIReturnKeyType", "UIAutocorrectionType"] {
        let key_string = get_static_str(env, key);
        if msg![env; coder containsValueForKey:key_string] {
            let value: NSInteger = msg![env; coder decodeIntegerForKey:key_string];
            match key {
                "UIBorderStyle" => { () = msg![env; this setBorderStyle:value]; }
                "UIKeyboardType" => { () = msg![env; this setKeyboardType:value]; }
                "UIReturnKeyType" => { () = msg![env; this setReturnKeyType:value]; }
                _ => { () = msg![env; this setAutocorrectionType:value]; }
            }
        }
    }

    this
}

- (())dealloc {
    let UITextFieldHostObject {
        superclass,
        text_label, placeholder_label, background_view, placeholder, attributed_placeholder, font, text_color,
        background, disabled_background, left_view, right_view, input_view, input_accessory_view, ..
    } = std::mem::take(env.objc.borrow_mut(this));

    release(env, text_label);
    release(env, placeholder_label);
    release(env, background_view);
    release(env, placeholder);
    release(env, attributed_placeholder);
    release(env, font);
    release(env, text_color);
    release(env, background);
    release(env, disabled_background);
    release(env, left_view);
    release(env, right_view);
    release(env, input_view);
    release(env, input_accessory_view);
    // Preserve superclass ownership until its own deallocator runs.
    env.objc.borrow_mut::<UITextFieldHostObject>(this).superclass = superclass;
    msg_super![env; this dealloc]
}

- (())layoutSubviews {
    update_background(env, this);
    let host = env.objc.borrow::<UITextFieldHostObject>(this);
    let (text_label, left, right, lm, rm, editing, placeholder, font, alignment) =
        (host.text_label, host.left_view, host.right_view, host.left_view_mode,
         host.right_view_mode, host.editing, host.placeholder, host.font, host.text_alignment);
    let bounds: CGRect = msg![env; this bounds];
    for (view, left_side, mode) in [(left, true, lm), (right, false, rm)] {
        if view == nil { continue; }
        let visible = accessory_visible(mode, editing);
        let parent: id = msg![env; view superview];
        if visible {
            if parent != this { () = msg![env; this addSubview:view]; }
            let rect: CGRect = if left_side { msg![env; this leftViewRectForBounds:bounds] }
                else { msg![env; this rightViewRectForBounds:bounds] };
            () = msg![env; view setFrame:rect];
        } else if parent == this {
            () = msg![env; view removeFromSuperview];
        }
    }
    let rect: CGRect = if editing {
        msg![env; this editingRectForBounds:bounds]
    } else {
        msg![env; this textRectForBounds:bounds]
    };
    let _: () = msg![env; text_label setFrame:rect];
    let mut label = env.objc.borrow::<UITextFieldHostObject>(this).placeholder_label;
    if placeholder != nil && label == nil {
        label = msg_class![env; UILabel new];
        let gray: id = msg_class![env; UIColor grayColor];
        let clear: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:clear];
        () = msg![env; label setTextColor:gray];
        () = msg![env; this addSubview:label];
        env.objc.borrow_mut::<UITextFieldHostObject>(this).placeholder_label = label;
    }
    if label != nil {
        let text: id = msg![env; text_label text];
        let length: NSUInteger = msg![env; text length];
        () = msg![env; label setHidden:(length != 0 || placeholder == nil)];
        () = msg![env; label setText:placeholder];
        () = msg![env; label setFont:font];
        () = msg![env; label setTextAlignment:alignment];
        let rect: CGRect = msg![env; this placeholderRectForBounds:bounds];
        () = msg![env; label setFrame:rect];
    }
}

- (id)text {
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    msg![env; text_label text]
}

- (())setText:(id)text {
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setText:text];
    () = msg![env; this layoutIfNeeded];
}

- (id)attributedText {
    let text: id = msg![env; this text];
    if text == nil { return nil; }
    msg_class![env; NSAttributedString alloc]
}

- (())setAttributedText:(id)attributed_text {
    if attributed_text == nil {
        let _: () = msg![env; this setText:nil];
        return;
    }
    let plain: id = msg![env; attributed_text string];
    let _: () = msg![env; this setText:plain];
}

- (id)placeholder { env.objc.borrow::<UITextFieldHostObject>(this).placeholder }

- (())setPlaceholder:(id)placeholder {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).placeholder;
    release(env, old);
    retain(env, placeholder);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).placeholder = placeholder;
    () = msg![env; this layoutIfNeeded];
}

- (id)attributedPlaceholder { env.objc.borrow::<UITextFieldHostObject>(this).attributed_placeholder }

- (())setAttributedPlaceholder:(id)attr_placeholder {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).attributed_placeholder;
    release(env, old);
    retain(env, attr_placeholder);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).attributed_placeholder = attr_placeholder;
    if attr_placeholder != nil {
        let plain: id = msg![env; attr_placeholder string];
        let _: () = msg![env; this setPlaceholder:plain];
    }
}

- (id)font { env.objc.borrow::<UITextFieldHostObject>(this).font }

- (())setFont:(id)new_font {
        // `-font` is never nil on iOS, and a nib that uses the default font
        // omits the key entirely, so fall back to the default instead of
        // storing nil: a nil font draws no text at all.
        let new_font: id = if new_font == nil {
            msg_class![env; UIFont systemFontOfSize:17.0]
        } else {
            new_font
        };
    let old = env.objc.borrow::<UITextFieldHostObject>(this).font;
    release(env, old);
    retain(env, new_font);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).font = new_font;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setFont:new_font];
    () = msg![env; this layoutIfNeeded];
}

- (id)textColor { env.objc.borrow::<UITextFieldHostObject>(this).text_color }

- (())setTextColor:(id)color {
        // Same for the colour: `-textColor` defaults to black, never nil.
        let color: id = if color == nil {
            msg_class![env; UIColor blackColor]
        } else {
            color
        };
    let old = env.objc.borrow::<UITextFieldHostObject>(this).text_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_color = color;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setTextColor:color];
}

- (UITextAlignment)textAlignment { env.objc.borrow::<UITextFieldHostObject>(this).text_alignment }

- (())setTextAlignment:(UITextAlignment)text_alignment {
    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_alignment = text_alignment;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setTextAlignment:text_alignment];
    () = msg![env; this layoutIfNeeded];
}

- (UITextBorderStyle)borderStyle { env.objc.borrow::<UITextFieldHostObject>(this).border_style }
- (())setBorderStyle:(UITextBorderStyle)style {
    env.objc.borrow_mut::<UITextFieldHostObject>(this).border_style = style;
    update_background(env, this);
    () = msg![env; this layoutIfNeeded];
}

- (())setEnabled:(bool)enabled {
    () = msg_super![env; this setEnabled:enabled];
    update_background(env, this);
}

- (id)background { env.objc.borrow::<UITextFieldHostObject>(this).background }
- (())setBackground:(id)background {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).background;
    release(env, old);
    retain(env, background);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).background = background;
    update_background(env, this);
}

- (id)disabledBackground { env.objc.borrow::<UITextFieldHostObject>(this).disabled_background }
- (())setDisabledBackground:(id)background {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).disabled_background;
    release(env, old);
    retain(env, background);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).disabled_background = background;
    update_background(env, this);
}

- (UITextFieldViewMode)clearButtonMode { env.objc.borrow::<UITextFieldHostObject>(this).clear_button_mode }
- (())setClearButtonMode:(UITextFieldViewMode)mode { env.objc.borrow_mut::<UITextFieldHostObject>(this).clear_button_mode = mode; }
- (())setClearsOnBeginEditing:(bool)clear { env.objc.borrow_mut::<UITextFieldHostObject>(this).clears_on_begin_editing = clear; }
- (bool)clearsOnBeginEditing { env.objc.borrow::<UITextFieldHostObject>(this).clears_on_begin_editing }

- (id)leftView { env.objc.borrow::<UITextFieldHostObject>(this).left_view }
- (())setLeftView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).left_view;
    retain(env, view);
    () = msg![env; old removeFromSuperview];
    release(env, old);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).left_view = view;
    if view != nil { () = msg![env; this addSubview:view]; }
    () = msg![env; this layoutIfNeeded];
}

- (UITextFieldViewMode)leftViewMode { env.objc.borrow::<UITextFieldHostObject>(this).left_view_mode }
- (())setLeftViewMode:(UITextFieldViewMode)mode {
    env.objc.borrow_mut::<UITextFieldHostObject>(this).left_view_mode = mode;
    () = msg![env; this layoutIfNeeded];
}

- (id)rightView { env.objc.borrow::<UITextFieldHostObject>(this).right_view }
- (())setRightView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).right_view;
    retain(env, view);
    () = msg![env; old removeFromSuperview];
    release(env, old);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).right_view = view;
    if view != nil { () = msg![env; this addSubview:view]; }
    () = msg![env; this layoutIfNeeded];
}
- (UITextFieldViewMode)rightViewMode { env.objc.borrow::<UITextFieldHostObject>(this).right_view_mode }
- (())setRightViewMode:(UITextFieldViewMode)mode {
    env.objc.borrow_mut::<UITextFieldHostObject>(this).right_view_mode = mode;
    () = msg![env; this layoutIfNeeded];
}

- (id)inputView { env.objc.borrow::<UITextFieldHostObject>(this).input_view }
- (())setInputView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).input_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).input_view = view;
}

- (id)inputAccessoryView { env.objc.borrow::<UITextFieldHostObject>(this).input_accessory_view }
- (())setInputAccessoryView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).input_accessory_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).input_accessory_view = view;
}

- (bool)adjustsFontSizeToFitWidth { env.objc.borrow::<UITextFieldHostObject>(this).adjusts_font_size_to_fit_width }
- (())setAdjustsFontSizeToFitWidth:(bool)adjusts { env.objc.borrow_mut::<UITextFieldHostObject>(this).adjusts_font_size_to_fit_width = adjusts; }
- (f32)minimumFontSize { env.objc.borrow::<UITextFieldHostObject>(this).minimum_font_size }
- (())setMinimumFontSize:(f32)size { env.objc.borrow_mut::<UITextFieldHostObject>(this).minimum_font_size = size; }

- (bool)isSecureTextEntry { env.objc.borrow::<UITextFieldHostObject>(this).secure_text_entry }
- (())setSecureTextEntry:(bool)secure { env.objc.borrow_mut::<UITextFieldHostObject>(this).secure_text_entry = secure; }
- (UITextAutocapitalizationType)autocapitalizationType { env.objc.borrow::<UITextFieldHostObject>(this).autocapitalization_type }
- (())setAutocapitalizationType:(UITextAutocapitalizationType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).autocapitalization_type = type_; }
- (UITextAutocorrectionType)autocorrectionType { env.objc.borrow::<UITextFieldHostObject>(this).autocorrection_type }
- (())setAutocorrectionType:(UITextAutocorrectionType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).autocorrection_type = type_; }
- (UIReturnKeyType)returnKeyType { env.objc.borrow::<UITextFieldHostObject>(this).return_key_type }
- (())setReturnKeyType:(UIReturnKeyType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).return_key_type = type_; }
- (UIKeyboardType)keyboardType { env.objc.borrow::<UITextFieldHostObject>(this).keyboard_type }
- (())setKeyboardType:(UIKeyboardType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).keyboard_type = type_; }
- (UIKeyboardAppearance)keyboardAppearance { env.objc.borrow::<UITextFieldHostObject>(this).keyboard_appearance }
- (())setKeyboardAppearance:(UIKeyboardAppearance)appearance { env.objc.borrow_mut::<UITextFieldHostObject>(this).keyboard_appearance = appearance; }
- (bool)enablesReturnKeyAutomatically { env.objc.borrow::<UITextFieldHostObject>(this).enables_return_key_automatically }
- (())setEnablesReturnKeyAutomatically:(bool)enables { env.objc.borrow_mut::<UITextFieldHostObject>(this).enables_return_key_automatically = enables; }
- (NSInteger)spellCheckingType { env.objc.borrow::<UITextFieldHostObject>(this).spell_checking_type }
- (())setSpellCheckingType:(NSInteger)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).spell_checking_type = type_; }
- (NSInteger)contentVerticalAlignment { env.objc.borrow::<UITextFieldHostObject>(this).content_vertical_alignment }
- (())setContentVerticalAlignment:(NSInteger)alignment { env.objc.borrow_mut::<UITextFieldHostObject>(this).content_vertical_alignment = alignment; }

- (id)delegate { env.objc.borrow::<UITextFieldHostObject>(this).delegate }
- (())setDelegate:(id)delegate { env.objc.borrow_mut::<UITextFieldHostObject>(this).delegate = delegate; }

- (bool)isEditing { env.objc.borrow::<UITextFieldHostObject>(this).editing }
- (bool)hasText {
    let text: id = msg![env; this text];
    if text == nil { return false; }
    let len: NSUInteger = msg![env; text length];
    len > 0
}

- (())deleteBackward {
    // UIKeyInput is dynamically dispatched, including when an engine keeps its
    // visible text outside the UITextField and overrides deletion on an empty field.
    // https://developer.apple.com/documentation/uikit/uikeyinput/deletebackward()
    handle_backspace(env, this);
}

- (())touchesBegan:(id)_touches withEvent:(id)_event { let _: bool = msg![env; this becomeFirstResponder]; }
- (bool)canBecomeFirstResponder { true }
- (bool)canResignFirstResponder { true }

- (bool)becomeFirstResponder {
    if env.objc.borrow::<UITextFieldHostObject>(this).editing {
        crate::frameworks::uikit::ui_keyboard::start_text_input(env);
        return true;
    }

    let delegate: id = env.objc.borrow::<UITextFieldHostObject>(this).delegate;
    let mut delegate_alive = false;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        delegate_alive = isa != 0;
    }

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldShouldBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] && !msg![env; delegate textFieldShouldBeginEditing:this] { return false; }
    }

    if env.objc.borrow::<UITextFieldHostObject>(this).clears_on_begin_editing {
        let empty = ns_string::get_static_str(env, "");
        let _: () = msg![env; this setText:empty];
    }

    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let curr_text: id = msg![env; text_label text];
    if curr_text == nil {
        let empty = ns_string::get_static_str(env, "");
        let _: () = msg![env; text_label setText:empty];
    }

    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UIKeyboardWillShowNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.framework_state.uikit.ui_responder.first_responder = this;
    crate::frameworks::uikit::ui_keyboard::start_text_input(env);

    let name = ns_string::get_static_str(env, UIKeyboardDidShowNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).editing = true;
    () = msg![env; this layoutIfNeeded];

        super::send_actions(env, this, nil, super::UIControlEventEditingDidBegin);

    let name = ns_string::get_static_str(env, UITextFieldTextDidBeginEditingNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldDidBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] { let _: () = msg![env; delegate textFieldDidBeginEditing:this]; }
    }
    true
}

- (bool)resignFirstResponder {
    if !env.objc.borrow::<UITextFieldHostObject>(this).editing { return true; }

    let delegate: id = env.objc.borrow::<UITextFieldHostObject>(this).delegate;
    let mut delegate_alive = false;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        delegate_alive = isa != 0;
    }

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldShouldEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] && !msg![env; delegate textFieldShouldEndEditing:this] { return false; }
    }

    // Resign before notifying observers. A keyboard-will-hide observer may
    // ask the same field to end editing again; it must see the committed state
    // rather than recursively beginning another keyboard dismissal.
    env.objc.borrow_mut::<UITextFieldHostObject>(this).editing = false;
    () = msg![env; this layoutIfNeeded];
    env.framework_state.uikit.ui_responder.first_responder = nil;

    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UIKeyboardWillHideNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    crate::frameworks::uikit::ui_keyboard::stop_text_input(env);

    let name = ns_string::get_static_str(env, UIKeyboardDidHideNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

        super::send_actions(env, this, nil, super::UIControlEventEditingDidEnd);

    let name = ns_string::get_static_str(env, UITextFieldTextDidEndEditingNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldDidEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] { let _: () = msg![env; delegate textFieldDidEndEditing:this]; }
    }
    true
}

- (CGRect)textRectForBounds:(CGRect)bounds { field_content_rect(env, this, bounds) }
- (CGRect)editingRectForBounds:(CGRect)bounds { field_content_rect(env, this, bounds) }
- (CGRect)placeholderRectForBounds:(CGRect)bounds { field_content_rect(env, this, bounds) }
- (CGRect)borderRectForBounds:(CGRect)bounds { bounds }

- (CGRect)clearButtonRectForBounds:(CGRect)bounds {
    let sz: f32 = 19.0;
    CGRect {
        origin: CGPoint {
            x: bounds.origin.x + bounds.size.width - sz - 4.0,
            y: bounds.origin.y + (bounds.size.height - sz) * 0.5,
        },
        size: CGSize { width: sz, height: sz },
    }
}

- (CGRect)leftViewRectForBounds:(CGRect)bounds {
    let view = env.objc.borrow::<UITextFieldHostObject>(this).left_view;
    let frame: CGRect = msg![env; view frame];
    CGRect { origin: CGPoint { x: bounds.origin.x,
        y: bounds.origin.y + (bounds.size.height - frame.size.height) / 2.0 }, size: frame.size }
}

- (CGRect)rightViewRectForBounds:(CGRect)bounds {
    let view = env.objc.borrow::<UITextFieldHostObject>(this).right_view;
    let frame: CGRect = msg![env; view frame];
    CGRect {
        origin: CGPoint { x: bounds.origin.x + bounds.size.width - frame.size.width,
            y: bounds.origin.y + (bounds.size.height - frame.size.height) / 2.0 },
        size: frame.size,
    }
}

- (())setPosition:(CGPoint)position { todo_objc_setter!(this, position); }

// MARK: - Smart typography (iOS 11+)

// RetroArch's iOS build calls these on its text fields at startup. They only
// configure how the on-screen keyboard would substitute characters, which the
// emulator does not model, so accepting and ignoring them matches the effect
// on a real device that has the feature switched off, and keeps the app from
// taking the "does not respond to selector" path.
- (())setSmartQuotesType:(NSInteger)_type {}
- (())setSmartDashesType:(NSInteger)_type {}
- (())setSmartInsertDeleteType:(NSInteger)_type {}
- (NSInteger)smartQuotesType { 0 }
- (NSInteger)smartDashesType { 0 }
- (NSInteger)smartInsertDeleteType { 0 }

@end

};

// Only accepted user edits send editing actions and notifications. Programmatic
// setters stay silent so observers can normalize text without recursive edits.
fn post_text_change(env: &mut Environment, field: id) {
    super::send_actions(env, field, nil, super::UIControlEventEditingChanged);
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UITextFieldTextDidChangeNotification);
    () = msg![env; center postNotificationName:name object:field userInfo:nil];
}

// Host IMEs report complete editing values (selection replacement, composing
// text and autocorrection), while UIKit delegates receive a UTF-16 NSRange.
// https://developer.apple.com/documentation/uikit/uitextfielddelegate/textfield(_:shouldchangecharactersin:replacementstring:)
#[allow(dead_code)] // entry point for a host with a real IME
pub fn handle_edit(env: &mut Environment, field: id, text: String) {
    let requested = text.clone();
    let current: id = msg![env; field text];
    let old = if current == nil { String::new() } else {
        ns_string::to_rust_string(env, current).into_owned()
    };
    if old == text { return; }
    let (range, replacement) = editing_change(&old, &text);
    let replacement = ns_string::from_rust_string(env, replacement);
    let delegate = env.objc.borrow::<UITextFieldHostObject>(field).delegate;
    let selector = env.objc.register_host_selector(
        "textField:shouldChangeCharactersInRange:replacementString:".to_string(), &mut env.mem);
    let accepted = delegate == nil || !msg![env; delegate respondsToSelector:selector]
        || msg![env; delegate textField:field shouldChangeCharactersInRange:range replacementString:replacement];
    if accepted {
        let value = ns_string::from_rust_string(env, text);
        let _: () = msg![env; field setText:value];
        post_text_change(env, field);
        let _: () = msg![env; field setNeedsDisplay];
        release(env, value);
    }
    release(env, replacement);
    // Delegates can reject a change after consuming its characters in an
    // engine-owned editor, or observers can normalize/clear the field. Return
    // that canonical value to the host editor so its next edit does not send
    // the previously consumed prefix again.
    // https://developer.apple.com/documentation/uikit/uitextfielddelegate/textfield(_:shouldchangecharactersin:replacementstring:)
    if env.framework_state.uikit.ui_responder.first_responder == field {
        let current: id = msg![env; field text];
        let current = if current == nil { String::new() } else {
            ns_string::to_rust_string(env, current).into_owned()
        };
        if current != requested {
            crate::frameworks::uikit::ui_keyboard::start_text_input(env);
        }
    }
}

#[allow(dead_code)]
fn editing_change(old: &str, new: &str) -> (NSRange, String) {
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a,b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev())
        .take_while(|(a,b)| a == b).count();
    (NSRange {
        location: old[..prefix].iter().map(|c| c.len_utf16() as u32).sum(),
        length: old[prefix..old.len()-suffix].iter().map(|c| c.len_utf16() as u32).sum(),
    }, new[prefix..new.len()-suffix].iter().collect())
}

pub fn handle_text(env: &mut Environment, text_field: id, text: String) {
    // Android's IME sends the "done"/return action as text ending in a newline
    // rather than as a separate return event. For an alert that is waiting for
    // input, that means "the user finished typing": insert whatever came
    // before the newline and confirm the alert.
    let (text, confirm) = match text.split_once('\n') {
        Some((head, _)) if ui_alert_view::is_active_text_field(env, text_field) => {
            (head.to_string(), true)
        }
        _ => (text, false),
    };
    // An empty replacement is a no-op keystroke. Appending it would post an
    // editing change for a change that never happened, which apps driving
    // their input loop from `UITextFieldTextDidChange` would act on.
    if text.is_empty() {
        if confirm { let _ = ui_alert_view::handle_return(env, text_field); }
        return;
    }
    let txt = ns_string::from_rust_string(env, text);
    let text_label = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .text_label;
    let mut curr_text: id = msg![env; text_label text];
    if curr_text == nil {
        curr_text = ns_string::get_static_str(env, "");
    }

    let len: NSUInteger = msg![env; curr_text length];
    let range = NSRange {
        location: len,
        length: 0,
    };
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    let mut should = true;

    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env.objc.register_host_selector(
                "textField:shouldChangeCharactersInRange:replacementString:".to_string(),
                &mut env.mem,
            );
            if msg![env; delegate respondsToSelector:sel] {
                should = msg![env; delegate textField:text_field shouldChangeCharactersInRange:range replacementString:txt];
            }
        }
    }

    if should {
        let new_text: id = msg![env; curr_text stringByAppendingString:txt];
        let _: () = msg![env; text_field setText:new_text];
        post_text_change(env, text_field);
        let _: () = msg![env; text_field setNeedsDisplay];
        release(env, new_text);
    }
    release(env, txt);
    if confirm {
        let _ = ui_alert_view::handle_return(env, text_field);
    }
}

pub fn handle_backspace(env: &mut Environment, text_field: id) {
    let text_label = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .text_label;
    let curr_text: id = msg![env; text_label text];
    let len: NSUInteger = msg![env; curr_text length];
    if len == 0 {
        return;
    }

    let range = NSRange {
        location: len - 1,
        length: 1,
    };
    let empty = ns_string::get_static_str(env, "");
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    let mut should = true;

    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env.objc.register_host_selector(
                "textField:shouldChangeCharactersInRange:replacementString:".to_string(),
                &mut env.mem,
            );
            if msg![env; delegate respondsToSelector:sel] {
                should = msg![env; delegate textField:text_field shouldChangeCharactersInRange:range replacementString:empty];
            }
        }
    }

    if should {
        let new_text: id = msg![env; curr_text substringToIndex:(len - 1)];
        let _: () = msg![env; text_field setText:new_text];
        post_text_change(env, text_field);
        let _: () = msg![env; text_field setNeedsDisplay];
    }
}

pub fn handle_return(env: &mut Environment, text_field: id) {
    // Delegate/action handlers may remove the sender from its view hierarchy.
    retain(env, text_field);
    // An alert that is waiting for text has no buttons to tap: its return key
    // stands in for the default button. See ui_alert_view::handle_return.
    if ui_alert_view::handle_return(env, text_field) {
        release(env, text_field);
        return;
    }
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env
                .objc
                .register_host_selector("textFieldShouldReturn:".to_string(), &mut env.mem);
            if msg![env; delegate respondsToSelector:sel] {
                let should_return: bool = msg![env; delegate textFieldShouldReturn:text_field];
                if !should_return {
                    release(env, text_field);
                    return;
                }
            }
        }
    }
    // A Return key is also a target-action event. Nib-backed forms often
    // connect this event without installing a text-field delegate.
    // https://developer.apple.com/documentation/uikit/uicontrol/event/editingdidendonexit
    super::send_actions(env, text_field, nil, super::UIControlEventEditingDidEndOnExit);
    release(env, text_field);
}

#[cfg(test)]
mod editing_tests {
    use super::*;

    #[test]
    fn accessories_follow_editing_modes_and_placeholder_never_changes_text() {
        // https://developer.apple.com/documentation/uikit/uitextfield/leftview
        // https://developer.apple.com/documentation/uikit/uitextfield/leftviewmode
        // https://developer.apple.com/documentation/uikit/uitextfield/placeholder
        // https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let bounds = CGRect { origin: CGPoint::default(), size: CGSize { width: 200.0, height: 40.0 } };
        let field: id = msg_class![env; UITextField alloc];
        let field: id = msg![env; field initWithFrame:bounds];
        let left: id = msg_class![env; UIView new];
        let right: id = msg_class![env; UIView new];
        let frame = CGRect { origin: CGPoint::default(), size: CGSize { width: 24.0, height: 20.0 } };
        () = msg![env; left setFrame:frame];
        () = msg![env; right setFrame:frame];
        () = msg![env; field setLeftView:left];
        () = msg![env; field setRightView:right];
        let placeholder = get_static_str(env, "Search");
        () = msg![env; field setPlaceholder:placeholder];
        for editing in [false, true] {
            env.objc.borrow_mut::<UITextFieldHostObject>(field).editing = editing;
            for mode in 0i32..=3 {
                () = msg![env; field setLeftViewMode:mode];
                () = msg![env; field setRightViewMode:mode];
                () = msg![env; field layoutSubviews];
                let parent: id = msg![env; left superview];
                let expected = matches!((mode, editing), (3, _) | (1, true) | (2, false));
                assert_eq!(parent == field, expected);
                let rect: CGRect = msg![env; field textRectForBounds:bounds];
                assert_eq!({ rect.size.width }, if expected { 152.0 } else { 200.0 });
                if expected {
                    let left_frame: CGRect = msg![env; left frame];
                    let right_frame: CGRect = msg![env; right frame];
                    assert_eq!({ left_frame.origin.y }, 10.0);
                    assert_eq!({ right_frame.origin.x }, 176.0);
                }
            }
        }
        let label = env.objc.borrow::<UITextFieldHostObject>(field).placeholder_label;
        let bg: id = msg![env; label backgroundColor];
        assert_eq!(crate::frameworks::uikit::ui_color::get_rgba(&env.objc, bg).3, 0.0);
        let hidden: bool = msg![env; label isHidden];
        assert!(!hidden);
        let text: id = msg![env; field text];
        assert_eq!(text, nil);
        () = msg![env; field setText:placeholder];
        () = msg![env; field layoutSubviews];
        let hidden: bool = msg![env; label isHidden];
        assert!(hidden);
        // UIKit's view mode must not undo an app's own visibility changes.
        () = msg![env; right setHidden:true];
        () = msg![env; field layoutSubviews];
        let hidden: bool = msg![env; right isHidden];
        assert!(hidden);
        () = msg![env; field setLeftView:nil];
        let parent: id = msg![env; left superview];
        assert_eq!(parent, nil);
        release(env, left);
        release(env, right);
        release(env, field);
        release(env, pool);
    }

    #[test]
    fn font_and_text_colour_are_never_nil() {
        // https://developer.apple.com/documentation/uikit/uitextfield/font
        // https://developer.apple.com/documentation/uikit/uitextfield/textcolor
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField alloc];
        let bounds: CGRect = CGRect::default();
        let field: id = msg![env; field initWithFrame:bounds];
        // A nib that uses the defaults omits both keys, so a decoded field
        // must still report the defaults rather than nil.
        let font: id = msg![env; field font];
        assert_ne!(font, nil);
        let colour: id = msg![env; field textColor];
        assert_ne!(colour, nil);
        () = msg![env; field setFont:nil];
        let font: id = msg![env; field font];
        assert_ne!(font, nil);
        () = msg![env; field setTextColor:nil];
        let colour: id = msg![env; field textColor];
        assert_ne!(colour, nil);
        release(env, pool);
    }

    fn inset_text(_: &mut Environment, _: id, _: SEL, mut bounds: CGRect) -> CGRect {
        bounds.origin.x += 4.0;
        bounds.size.width -= 8.0;
        bounds
    }
    fn inset_editing(_: &mut Environment, _: id, _: SEL, mut bounds: CGRect) -> CGRect {
        bounds.origin.x += 12.0;
        bounds.size.width -= 24.0;
        bounds
    }
    #[test]
    fn borderless_fields_are_transparent_and_use_overridden_text_rectangles() {
        // https://developer.apple.com/documentation/uikit/uiview/backgroundcolor
        // https://developer.apple.com/documentation/uikit/uitextfield/editingrect(forbounds:)
        // https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let class = env.objc.get_known_class("UITextField", &mut env.mem);
        type RectMethod = fn(&mut Environment, id, SEL, CGRect) -> CGRect;
        static TEXT_RECT: RectMethod = inset_text;
        static EDITING_RECT: RectMethod = inset_editing;
        for (name, method) in [("textRectForBounds:", &TEXT_RECT),
                               ("editingRectForBounds:", &EDITING_RECT)] {
            let selector = env.objc.register_host_selector(name.into(), &mut env.mem);
            let imp = env.dyld.create_guest_function(&mut env.mem, "[text rectangle probe]", method);
            let types = env.mem.alloc_and_write_cstr(b"{CGRect={CGPoint=ff}{CGSize=ff}}@:{CGRect={CGPoint=ff}{CGSize=ff}}");
            crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        }
        let field: id = msg_class![env; UITextField alloc];
        let frame = CGRect { origin: CGPoint::default(), size: CGSize { width: 120.0, height: 32.0 } };
        let field: id = msg![env; field initWithFrame:frame];
        let background: id = msg![env; field backgroundColor];
        assert_eq!(background, nil);
        let opaque: bool = msg![env; field isOpaque];
        assert!(!opaque);
        let label = env.objc.borrow::<UITextFieldHostObject>(field).text_label;
        for (editing, expected) in [(false, 4.0), (true, 12.0), (false, 4.0)] {
            if editing { assert!(msg![env; field becomeFirstResponder]); }
            else { assert!(msg![env; field resignFirstResponder]); }
            () = msg![env; field layoutIfNeeded];
            let actual: CGRect = msg![env; label frame];
            let x = actual.origin.x;
            assert_eq!(x, expected);
        }
        release(env, field);
        release(env, pool);
    }

    fn record_delete_key(env: &mut Environment, field: id, _: SEL) {
        let count: NSInteger = msg![env; field tag];
        () = msg![env; field setTag:(count + 1)];
    }

    #[test]
    fn delete_backward_dispatches_overrides_even_with_an_empty_proxy() {
        // UIKeyInput deletion belongs to the responder, whose backing store
        // need not be UITextField.text (e.g. a guest engine's visible editor).
        // https://developer.apple.com/documentation/uikit/uikeyinput/deletebackward()
        // Upstream currently handles keys through host helpers instead:
        // https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField new];
        let value = ns_string::get_static_str(env, "A");
        () = msg![env; field setText:value];
        () = msg![env; field deleteBackward];
        let value: id = msg![env; field text];
        assert_eq!(ns_string::to_rust_string(env, value), "");
        let class = env.objc.get_known_class("UITextField", &mut env.mem);
        let selector = env.objc.register_host_selector("deleteBackward".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL);
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[custom deletion]", &(record_delete_key as Callback));
        let types = env.mem.alloc_and_write_cstr(b"v@:");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        crate::frameworks::uikit::ui_responder::handle_text_input_event(
            env, field, crate::window::TextInputEvent::Backspace);
        let count: NSInteger = msg![env; field tag];
        assert_eq!(count, 1);
        release(env, field);
        release(env, pool);
    }

    #[test]
    fn background_images_follow_border_and_enabled_state_without_covering_text() {
        use crate::frameworks::core_graphics::{cg_image, CGFloat};

        fn image(env: &mut Environment, pixel: Vec<u8>) -> id {
            let cg = cg_image::from_image(env, crate::image::Image::from_pixels(1, 1, pixel));
            let image: id = msg_class![env; UIImage alloc];
            let image: id = msg![env; image initWithCGImage:cg];
            cg_image::CGImageRelease(env, cg);
            image
        }

        fn assert_image(env: &mut Environment, view: id, expected: id) {
            let actual: id = msg![env; view image];
            assert_eq!(actual, expected);
            let layer: id = msg![env; view layer];
            let contents: id = msg![env; layer contents];
            let expected_contents: id = msg![env; expected CGImage];
            assert_eq!(contents, expected_contents);
        }

        // Rounded borders ignore custom images. Other border styles display
        // the normal or disabled image behind the field's editable text.
        // https://developer.apple.com/documentation/uikit/uitextfield/background
        // https://developer.apple.com/documentation/uikit/uitextfield/disabledbackground
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField new];
        let normal = image(env, vec![255, 0, 0, 255]);
        let disabled = image(env, vec![0, 0, 255, 255]);
        let text = ns_string::get_static_str(env, "18");
        () = msg![env; field setText:text];
        () = msg![env; field setBorderStyle:(3i32)];
        () = msg![env; field setBackground:normal];
        () = msg![env; field setDisabledBackground:disabled];
        let background = env.objc.borrow::<UITextFieldHostObject>(field).background_view;
        assert_ne!(background, nil);
        let layer: id = msg![env; background layer];
        let border: CGFloat = msg![env; layer borderWidth];
        let radius: CGFloat = msg![env; layer cornerRadius];
        assert!(border > 0.0 && radius > 0.0);
        assert_image(env, background, nil);
        () = msg![env; field setEnabled:false];
        assert_image(env, background, nil);

        () = msg![env; field setBorderStyle:(0i32)];
        assert_image(env, background, disabled);
        let border: CGFloat = msg![env; layer borderWidth];
        let radius: CGFloat = msg![env; layer cornerRadius];
        assert_eq!((border, radius), (0.0, 0.0));
        () = msg![env; field setEnabled:true];
        assert_image(env, background, normal);

        let frame = CGRect {
            origin: CGPoint { x: 9.0, y: 11.0 },
            size: CGSize { width: 72.0, height: 36.0 },
        };
        () = msg![env; field setFrame:frame];
        () = msg![env; field layoutSubviews];
        let background_frame: CGRect = msg![env; background frame];
        let bounds: CGRect = msg![env; field bounds];
        assert_eq!(background_frame, bounds);
        let interactive: bool = msg![env; background isUserInteractionEnabled];
        assert!(!interactive);
        let subviews: id = msg![env; field subviews];
        let count: NSUInteger = msg![env; subviews count];
        let bottom: id = msg![env; subviews objectAtIndex:0u32];
        let top: id = msg![env; subviews objectAtIndex:1u32];
        assert_eq!(count, 2);
        assert_eq!(bottom, background);
        assert_eq!(top, env.objc.borrow::<UITextFieldHostObject>(field).text_label);
        let displayed_text: id = msg![env; top text];
        assert_eq!(ns_string::to_rust_string(env, displayed_text), "18");

        release(env, field);
        release(env, normal);
        release(env, disabled);
        release(env, pool);
    }

    thread_local! {
        static CHANGE_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static HIDE_STATES: std::cell::RefCell<Vec<(bool, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn resign_during_keyboard_hide(env: &mut Environment, _: id, _: SEL, notification: id) {
        let field: id = msg![env; notification object];
        let editing: bool = msg![env; field isEditing];
        let first: bool = msg![env; field isFirstResponder];
        let count = HIDE_STATES.with(|states| {
            let mut states = states.borrow_mut();
            states.push((editing, first));
            states.len()
        });
        if count <= 3 {
            let _: bool = msg![env; field resignFirstResponder];
        }
    }

    #[test]
    fn keyboard_hide_observer_can_end_editing_again_without_recursion() {
        // Resignation precedes keyboard hiding; an observer ending editing
        // again sees a field which has already resigned and cannot restart it.
        // https://developer.apple.com/documentation/uikit/uitextfielddelegate
        // https://developer.apple.com/documentation/uikit/uiresponder/resignfirstresponder()
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField alloc];
        let field: id = msg![env; field initWithFrame:(CGRect::default())];
        let observer: id = msg_class![env; NSObject new];
        let class: crate::objc::Class = msg_class![env; NSObject class];
        let selector = env.objc.register_host_selector("editingTestWillHide:".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL, id);
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[keyboard hiding observer]", &(resign_during_keyboard_hide as Callback));
        let types = env.mem.alloc_and_write_cstr(b"v@:@");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let name = ns_string::get_static_str(env, UIKeyboardWillHideNotification);
        () = msg![env; center addObserver:observer selector:selector name:name object:field];
        assert!(msg![env; field becomeFirstResponder]);
        HIDE_STATES.with(|states| states.borrow_mut().clear());
        assert!(msg![env; field resignFirstResponder]);
        HIDE_STATES.with(|states| assert_eq!(*states.borrow(), vec![(false, false)]));
        assert!(msg![env; field resignFirstResponder]);
        HIDE_STATES.with(|states| assert_eq!(states.borrow().len(), 1));
        () = msg![env; center removeObserver:observer];
        release(env, observer);
        release(env, field);
        release(env, pool);
    }

    fn normalize_text(env: &mut Environment, _: id, _: SEL, notification: id) {
        let count = CHANGE_COUNT.with(|count| {
            count.set(count.get() + 1);
            count.get()
        });
        // Bound the red test without hiding recursive notifications.
        if count <= 3 {
            let field: id = msg![env; notification object];
            let normalized = ns_string::get_static_str(env, "30");
            () = msg![env; field setText:normalized];
        }
    }

    fn reject_host_change(_: &mut Environment, _: id, _: SEL, _: id, _: NSRange, _: id) -> bool {
        false
    }

    #[test]
    fn rejected_host_edits_restore_the_canonical_editor_value() {
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField new];
        let delegate: id = msg_class![env; NSObject new];
        let class: crate::objc::Class = msg_class![env; NSObject class];
        let selector = env.objc.register_host_selector(
            "textField:shouldChangeCharactersInRange:replacementString:".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL, id, NSRange, id) -> bool;
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[reject host edit]", &(reject_host_change as Callback));
        let types = env.mem.alloc_and_write_cstr(b"B@:@{_NSRange=II}@");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        () = msg![env; field setDelegate:delegate];
        let value = ns_string::get_static_str(env, "A");
        () = msg![env; field setText:value];
        env.framework_state.uikit.ui_responder.first_responder = field;
        // A delegate veto must leave the field value alone.
        let actual: id = msg![env; field text];
        assert_eq!(ns_string::to_rust_string(env, actual), "A");
        env.framework_state.uikit.ui_responder.first_responder = nil;
        release(env, delegate);
        release(env, field);
        release(env, pool);
    }

    fn save_edited_text(env: &mut Environment, target: id, _: SEL, field: id) {
        let count: NSInteger = msg![env; target tag];
        () = msg![env; target setTag:(count + 1)];
        let text: id = msg![env; field text];
        let defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
        let key = ns_string::get_static_str(env, "editingActionTest");
        () = msg![env; defaults setObject:text forKey:key];
    }

    #[test]
    fn user_editing_actions_save_the_field_value_once_per_change() {
        // Target-action consumers receive the text field itself, and can save
        // its value without observing NSNotificationCenter. setText: is silent.
        // https://developer.apple.com/documentation/uikit/uicontrol/event/editingchanged
        // https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let _application: id = msg_class![env; UIApplication new];
        let field: id = msg_class![env; UITextField new];
        let value = ns_string::get_static_str(env, "18");
        () = msg![env; field setText:value];
        let target: id = msg_class![env; UIView new];
        let class: crate::objc::Class = msg_class![env; UIView class];
        let selector = env.objc.register_host_selector("editingTestSave:".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL, id);
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[editing action target]", &(save_edited_text as Callback));
        let types = env.mem.alloc_and_write_cstr(b"v@:@");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        let events = super::super::UIControlEventEditingChanged;
        () = msg![env; field addTarget:target action:selector forControlEvents:events];
        let defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
        let key = ns_string::get_static_str(env, "editingActionTest");
        let initial = ns_string::get_static_str(env, "Ada");
        () = msg![env; field setText:initial];
        let count: NSInteger = msg![env; target tag];
        assert_eq!(count, 0);
        for (count, expected) in [(1, "Ada1"), (2, "Ada"), (3, "Grace")] {
            match count {
                1 => handle_text(env, field, "1".into()),
                2 => handle_backspace(env, field),
                _ => handle_edit(env, field, expected.into()),
            }
            let actual_count: NSInteger = msg![env; target tag];
            assert_eq!(actual_count, count);
            let saved: id = msg![env; defaults objectForKey:key];
            assert_ne!(saved, nil);
            assert_eq!(ns_string::to_rust_string(env, saved), expected);
        }
        handle_edit(env, field, "Grace".into());
        let count: NSInteger = msg![env; target tag];
        assert_eq!(count, 3);
        release(env, target);
        release(env, field);
        release(env, pool);
    }

    #[test]
    fn programmatic_text_is_silent_and_user_edits_notify_once() {
        // Notifications describe user editing; an observer may normalize text
        // with setText: without recursively starting another editing event.
        // https://developer.apple.com/documentation/uikit/uitextfielddelegate
        // https://developer.apple.com/documentation/uikit/uitextfield/textdidchangenotification
        // Upstream's setter explicitly left notification semantics as TODO:
        // https://github.com/touchHLE/touchHLE/blob/trunk/src/frameworks/uikit/ui_view/ui_control/ui_text_field.rs
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField alloc];
        let field: id = msg![env; field initWithFrame:(CGRect::default())];
        let observer: id = msg_class![env; NSObject new];
        let class: crate::objc::Class = msg_class![env; NSObject class];
        let selector = env.objc.register_host_selector("editingTestChanged:".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL, id);
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[text editing observer]", &(normalize_text as Callback));
        let types = env.mem.alloc_and_write_cstr(b"v@:@");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let name = ns_string::get_static_str(env, UITextFieldTextDidChangeNotification);
        () = msg![env; center addObserver:observer selector:selector name:name object:field];
        CHANGE_COUNT.with(|count| count.set(0));
        let value = ns_string::get_static_str(env, "20");
        () = msg![env; field setText:value];
        assert_eq!(CHANGE_COUNT.with(|count| count.get()), 0, "setText: emitted an editing notification");
        handle_text(env, field, "1".into());
        assert_eq!(CHANGE_COUNT.with(|count| count.get()), 1);
        handle_backspace(env, field);
        assert_eq!(CHANGE_COUNT.with(|count| count.get()), 2);
        handle_edit(env, field, "45".into());
        assert_eq!(CHANGE_COUNT.with(|count| count.get()), 3);
        handle_edit(env, field, "30".into());
        assert_eq!(CHANGE_COUNT.with(|count| count.get()), 3, "unchanged host value emitted a notification");
        let actual: id = msg![env; field text];
        assert_eq!(ns_string::to_rust_string(env, actual), "30");
        () = msg![env; center removeObserver:observer];
        release(env, observer);
        release(env, field);
        release(env, pool);
    }

    #[test]
    fn return_key_delivers_the_nib_exit_action_without_a_delegate() {
        // Forms can connect EditingDidEndOnExit in Interface Builder instead
        // of implementing UITextFieldDelegate.
        // https://developer.apple.com/documentation/uikit/uicontrol/event/editingdidendonexit
        let mut environment = crate::environment::touch_test_environment();
        let env = &mut environment;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let _application: id = msg_class![env; UIApplication new];
        let field: id = msg_class![env; UITextField new];
        let target: id = msg_class![env; UIView new];
        let class: crate::objc::Class = msg_class![env; UIView class];
        let selector = env.objc.register_host_selector("textFieldTestExit:".into(), &mut env.mem);
        type Callback = fn(&mut Environment, id, SEL, id);
        let imp = env.dyld.create_guest_function(
            &mut env.mem, "[text field exit action]", &(save_edited_text as Callback));
        let types = env.mem.alloc_and_write_cstr(b"v@:@");
        crate::objc::class_replaceMethod(env, class, selector, imp.to_ptr(), types.cast_const());
        () = msg![env; field addTarget:target action:selector
            forControlEvents:(super::super::UIControlEventEditingDidEndOnExit)];
        handle_return(env, field);
        let count: NSInteger = msg![env; target tag];
        assert_eq!(count, 1);
        release(env, target);
        release(env, field);
        release(env, pool);
    }

    #[test]
    fn host_edits_replace_selections_and_report_utf16_ranges() {
        let (range, replacement) = editing_change("A😺 cat", "A🐈 cat");
        assert_eq!((range.location, range.length), (1, 2));
        assert_eq!(replacement, "🐈");
        let mut env = crate::environment::touch_test_environment();
        let env = &mut env;
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let field: id = msg_class![env; UITextField alloc];
        let field: id = msg![env; field initWithFrame:(CGRect::default())];
        for value in ["Tom", "Timo", "😺", ""] {
            handle_edit(env, field, value.to_string());
            let actual: id = msg![env; field text];
            assert_eq!(ns_string::to_rust_string(env, actual), value);
        }
        release(env, field);
        release(env, pool);
    }
}
