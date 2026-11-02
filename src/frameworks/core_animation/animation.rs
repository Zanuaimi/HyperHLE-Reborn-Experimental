/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Functions, traits, and all kinds of things to assist with bridging the gap
//! between guest and host when it comes to animations in Core Animation.
//! Based in Apple's documented behavior for Core Animation, although not an
//! exact match.
//! References:
//! - Core Animation Programming Guide
//!   <https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/Introduction/Introduction.html>
//! - List of Animatable properties
//!   <https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/AnimatableProperties/AnimatableProperties.html#//apple_ref/doc/uid/TP40004514-CH11-SW2>
//! - Animation timing behavior, layers' local time, autoreverses, etc.
//!   <https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/AdvancedAnimationTricks/AdvancedAnimationTricks.html>
//! - Algorithm for choosing interpolation values
//!   <https://developer.apple.com/documentation/quartzcore/cabasicanimation?language=objc>
//! - Keyframe values, key times, and calculation modes
//!   <https://developer.apple.com/documentation/quartzcore/cakeyframeanimation/values>
//!   <https://developer.apple.com/documentation/quartzcore/cakeyframeanimation/keytimes>
//!   <https://developer.apple.com/documentation/quartzcore/caanimationcalculationmode>
use std::ops::{Add, Mul, Sub};

use crate::frameworks::core_animation::ca_animation::{
    get_animation_start_time, kCAAnimationDiscrete, kCAAnimationLinear, kCAFillModeBackwards,
    kCAFillModeBoth, kCAFillModeForwards, CAMediaTimingFillMode,
};
use crate::frameworks::core_animation::ca_layer::remove_anonymous_animation;
use crate::frameworks::core_animation::ca_transform3d::CATransform3D;
use crate::frameworks::core_animation::{ca_layer::CALayerHostObject, CACurrentMediaTime};
use crate::frameworks::core_foundation::time::CFTimeInterval;
use crate::frameworks::core_graphics::cg_color::CGColorHostObject;
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::objc::{id, msg, nil, release, retain};
use crate::Environment;

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyframeSegment {
    from_index: usize,
    to_index: usize,
    amount: f32,
}

fn select_keyframe_segment(
    progress: f32,
    key_times: Option<&[f32]>,
    value_count: usize,
    discrete: bool,
) -> Option<KeyframeSegment> {
    if value_count == 0 {
        return None;
    }

    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let times: Vec<f32> = match key_times {
        Some(times)
            if times.len() == value_count
                && times.first().copied() == Some(0.0)
                && times.last().copied() == Some(1.0)
                && times.iter().all(|time| time.is_finite())
                && times.windows(2).all(|pair| pair[0] < pair[1]) =>
        {
            times.to_vec()
        }
        _ if value_count == 1 => vec![0.0],
        _ => (0..value_count)
            .map(|index| index as f32 / (value_count - 1) as f32)
            .collect(),
    };

    if value_count == 1 {
        return Some(KeyframeSegment {
            from_index: 0,
            to_index: 0,
            amount: 0.0,
        });
    }

    if discrete {
        let index = times
            .iter()
            .rposition(|time| *time <= progress)
            .unwrap_or(0);
        return Some(KeyframeSegment {
            from_index: index,
            to_index: index,
            amount: 0.0,
        });
    }

    let from_index = if progress >= 1.0 {
        value_count - 1
    } else {
        times
            .windows(2)
            .position(|pair| progress < pair[1])
            .unwrap_or(value_count - 2)
    };
    let to_index = (from_index + 1).min(value_count - 1);
    let amount = if from_index == to_index {
        0.0
    } else {
        ((progress - times[from_index]) / (times[to_index] - times[from_index])).clamp(0.0, 1.0)
    };

    Some(KeyframeSegment {
        from_index,
        to_index,
        amount,
    })
}

fn interpolate_keyframe<T>(from: T, to: T, amount: f32) -> T
where
    T: Copy + Add<Output = T> + Mul<f32, Output = T> + Sub<Output = T>,
{
    from + (to - from) * amount
}

fn interpolate_values<T>(
    current: T,
    basic_from: Option<T>,
    basic_to: Option<T>,
    basic_by: Option<T>,
    keyframe_from: Option<T>,
    keyframe_to: Option<T>,
    amount: f32,
) -> T
where
    T: Copy + Add<Output = T> + Mul<f32, Output = T> + Sub<Output = T>,
{
    if let Some(from) = keyframe_from {
        interpolate_keyframe(from, keyframe_to.unwrap_or(from), amount)
    } else {
        let (from, by) = get_from_and_by_values(Some(current), basic_from, basic_to, basic_by);
        from + by * amount
    }
}

fn apply_keyframe_timing_function(
    env: &mut Environment,
    timing_functions: id,
    segment: KeyframeSegment,
    amount: f32,
) -> f32 {
    if timing_functions == nil || segment.amount == 0.0 || segment.amount == 1.0 {
        return amount;
    }

    let function_count: usize = msg![env; timing_functions count];
    if segment.from_index >= function_count {
        return amount;
    }

    let function_index = segment.from_index;
    let function: id = msg![env; timing_functions objectAtIndex:function_index];
    let function_class = msg![env; function class];
    let Some(timing_function_class) = env
        .objc
        .try_get_known_class("CAMediaTimingFunction", &mut env.mem)
    else {
        return amount;
    };
    if env
        .objc
        .class_is_subclass_of(function_class, timing_function_class)
    {
        msg![env; function _solveForInput: amount]
    } else {
        amount
    }
}

#[derive(Default)]
pub struct State {
    started_animations: Vec<id>,
    finished_animations: Vec<(id, id, bool, bool, Option<String>)>,
}
impl State {
    pub fn create_presentation_layer(
        &mut self,
        env: &mut Environment,
        layer: id,
    ) -> CALayerHostObject {
        // Clone given layer
        let original = env.objc.borrow::<CALayerHostObject>(layer);
        let model_transform = original.transform_3d;
        let mut presentation = original.clone();

        // Loop over all animations and set the presentation layer's values
        let named_animations: Vec<(Option<String>, id)> = presentation
            .animations
            .iter()
            .map(|(key, anim)| (Some(key.clone()), *anim))
            .collect();
        let anonymous_animations: Vec<(Option<String>, id)> = presentation
            .anonymous_animations
            .iter()
            .map(|anim| (None, *anim))
            .collect();

        for (key, animation) in
            Iterator::chain(named_animations.iter(), anonymous_animations.iter())
        {
            let animation = *animation;

            let fill_mode: CAMediaTimingFillMode = msg![env; animation fillMode];
            let fill_mode = to_rust_string(env, fill_mode);

            // TODO: Convert to local time
            let current_time = CACurrentMediaTime(env);
            let begin_time: CFTimeInterval = msg![env; animation beginTime];
            let start_time = get_animation_start_time(env, animation);

            if current_time >= begin_time {
                if start_time.is_none() {
                    // Animation started but isn't marked as such
                    start_time.replace(current_time);
                    self.started_animations.push(animation);
                }
            } else if fill_mode != kCAFillModeBackwards && fill_mode != kCAFillModeBoth {
                continue;
            }

            let effective_begin_time = start_time.unwrap_or(begin_time);

            if let Some(key) = key {
                log_dbg!(
                    "Animate CALayer {:?} animation {} {:?}",
                    layer,
                    key,
                    animation
                );
            } else {
                log_dbg!("Animate CALayer {:?} animation {:?}", layer, animation);
            }

            let repeat_count: f32 = msg![env; animation repeatCount];
            // Per Apple's docs, setting `repeatCount` to a very large number
            // (idiomatically `HUGE_VALF`, i.e. +infinity, or
            // `greatestFiniteMagnitude`, i.e. f32::MAX) makes the animation
            // repeat forever. A positive infinity is the most common value
            // games use, so it must be treated as "forever" rather than being
            // rejected as invalid (which would clamp it to a single play-through
            // and stop looping animations after one cycle).
            let effective_repeat_count =
                if repeat_count == f32::INFINITY || repeat_count == f32::MAX {
                    f32::INFINITY
                } else if repeat_count.is_finite() && repeat_count > 0.0 {
                    repeat_count
                } else {
                    // 0 means "no explicit repeat" (play once); a negative or NaN
                    // value is undefined per the docs, so also fall back to one
                    // play-through instead of crashing the host.
                    if repeat_count.is_nan() || repeat_count < 0.0 {
                        log!(
                            "Warning: CABasicAnimation: invalid repeatCount {}; treating as 1.",
                            repeat_count
                        );
                    }
                    1.0
                };

            let duration: CFTimeInterval = msg![env; animation duration];
            let current_repeat = (((current_time - effective_begin_time).max(0.0) / duration)
                as f32)
                .min(effective_repeat_count);

            let mut progress = current_repeat.fract();
            if current_repeat >= effective_repeat_count {
                progress = 1.0;
            }

            let autoreverses: bool = msg![env; animation autoreverses];
            if autoreverses {
                // From the docs:
                // Setting the repeat count to a whole number (such as 1.0) for
                // an autoreversing animation causes the animation to stop on
                // its starting value.
                // Adding an extra half step (such as a repeat count of 1.5)
                // causes the animation to stop on its end value
                progress = ((progress * 2.0 - 1.0).abs() - 1.0).abs();
            }

            let animation_class = msg![env; animation class];
            let is_keyframe = match env
                .objc
                .try_get_known_class("CAKeyframeAnimation", &mut env.mem)
            {
                Some(keyframe_class) => env
                    .objc
                    .class_is_subclass_of(animation_class, keyframe_class),
                None => false,
            };
            let timing_function_amount = if is_keyframe {
                progress
            } else {
                let timing_function: id = msg![env; animation timingFunction];
                msg![env; timing_function _solveForInput: progress]
            };

            if current_repeat >= effective_repeat_count {
                let removed_on_completion: bool = msg![env; animation isRemovedOnCompletion];
                self.finished_animations.push((
                    layer,
                    animation,
                    true,
                    removed_on_completion,
                    key.to_owned(),
                ));
                if fill_mode != kCAFillModeForwards && fill_mode != kCAFillModeBoth {
                    continue;
                }
            }

            let mut keyframe_from = None;
            let mut keyframe_to = None;
            let (from_value, to_value, by_value, interpolation_amount) = if is_keyframe {
                let values: id = msg![env; animation values];
                if values == nil {
                    continue;
                }
                let value_count: usize = msg![env; values count];
                let key_times: id = msg![env; animation keyTimes];
                let key_times = if key_times == nil {
                    None
                } else {
                    let count: usize = msg![env; key_times count];
                    if count != value_count {
                        None
                    } else {
                        Some(
                            (0..count)
                                .map(|index| {
                                    let value: id = msg![env; key_times objectAtIndex:index];
                                    msg![env; value floatValue]
                                })
                                .collect::<Vec<f32>>(),
                        )
                    }
                };
                let calculation_mode: id = msg![env; animation calculationMode];
                let calculation_mode = if calculation_mode == nil {
                    kCAAnimationLinear.to_owned()
                } else {
                    to_rust_string(env, calculation_mode).into_owned()
                };
                let calculation_mode = calculation_mode.as_str();
                if calculation_mode != kCAAnimationDiscrete
                    && calculation_mode != kCAAnimationLinear
                {
                    log_dbg!(
                        "Skipping unsupported CAKeyframeAnimation calculation mode {:?}",
                        calculation_mode
                    );
                    continue;
                }
                let discrete_mode = calculation_mode == kCAAnimationDiscrete;
                let segment = match select_keyframe_segment(
                    progress,
                    key_times.as_deref(),
                    value_count,
                    discrete_mode,
                ) {
                    Some(segment) => segment,
                    None => continue,
                };
                let segment_timing_functions: id = msg![env; animation timingFunctions];
                if segment.from_index == segment.to_index {
                    let index = segment.from_index;
                    keyframe_from = Some(msg![env; values objectAtIndex:index]);
                } else {
                    let from_index = segment.from_index;
                    let to_index = segment.to_index;
                    keyframe_from = Some(msg![env; values objectAtIndex:from_index]);
                    keyframe_to = Some(msg![env; values objectAtIndex:to_index]);
                }
                let amount = if discrete_mode {
                    0.0
                } else {
                    apply_keyframe_timing_function(
                        env,
                        segment_timing_functions,
                        segment,
                        segment.amount,
                    )
                };
                (nil, nil, nil, amount)
            } else {
                (
                    msg![env; animation fromValue],
                    msg![env; animation toValue],
                    msg![env; animation byValue],
                    timing_function_amount,
                )
            };
            // Update values only in the presentation layer
            let key_path: id = msg![env; animation keyPath];
            let key_path = to_rust_string(env, key_path);
            // Only these properties are animatable
            // TODO: Implement for all properties
            match &*key_path {
                "anchorPoint" => {
                    let from_value =
                        id_as_option(from_value).map(|obj| msg![env; obj CGPointValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj CGPointValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj CGPointValue]);
                    presentation.anchor_point = interpolate_values(
                        presentation.anchor_point,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from.map(|value| msg![env; value CGPointValue]),
                        keyframe_to.map(|value| msg![env; value CGPointValue]),
                        interpolation_amount,
                    );
                }
                "backgroundColor" => {
                    let from_value = id_as_option(from_value)
                        .map(|obj| *env.objc.borrow::<CGColorHostObject>(obj));
                    let to_value = id_as_option(to_value)
                        .map(|obj| *env.objc.borrow::<CGColorHostObject>(obj));
                    let by_value = id_as_option(by_value)
                        .map(|obj| *env.objc.borrow::<CGColorHostObject>(obj));
                    let keyframe_from_value =
                        keyframe_from.map(|obj| *env.objc.borrow::<CGColorHostObject>(obj));
                    let keyframe_to_value =
                        keyframe_to.map(|obj| *env.objc.borrow::<CGColorHostObject>(obj));
                    if let Some(from) = keyframe_from_value {
                        presentation.background_color = Some(interpolate_keyframe(
                            from,
                            keyframe_to_value.unwrap_or(from),
                            interpolation_amount,
                        ));
                    } else {
                        let (from_value, by_value) = get_from_and_by_values(
                            presentation.background_color,
                            from_value,
                            to_value,
                            by_value,
                        );
                        presentation.background_color =
                            Some(from_value + by_value * interpolation_amount);
                    }
                }
                "bounds" => {
                    let from_value = id_as_option(from_value).map(|obj| msg![env; obj CGRectValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj CGRectValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj CGRectValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj CGRectValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj CGRectValue]);
                    presentation.bounds = interpolate_values(
                        presentation.bounds,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                }
                "cornerRadius" => {
                    let from_value = id_as_option(from_value).map(|obj| msg![env; obj floatValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj floatValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj floatValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj floatValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj floatValue]);
                    presentation.corner_radius = interpolate_values(
                        presentation.corner_radius,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                }
                "hidden" => {
                    let from_value = id_as_option(from_value)
                        .map(|obj| msg![env; obj boolValue])
                        .map(|val: bool| val as i32 as f32);
                    let to_value = id_as_option(to_value)
                        .map(|obj| msg![env; obj boolValue])
                        .map(|val: bool| val as i32 as f32);
                    let by_value = id_as_option(by_value)
                        .map(|obj| msg![env; obj boolValue])
                        .map(|val: bool| val as i32 as f32);
                    let keyframe_from_value = keyframe_from
                        .map(|obj| msg![env; obj boolValue])
                        .map(|val: bool| val as i32 as f32);
                    let keyframe_to_value = keyframe_to
                        .map(|obj| msg![env; obj boolValue])
                        .map(|val: bool| val as i32 as f32);
                    presentation.hidden = interpolate_values(
                        presentation.hidden as i32 as f32,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    ) > 0.5;
                }
                "opacity" => {
                    let from_value = id_as_option(from_value).map(|obj| msg![env; obj floatValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj floatValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj floatValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj floatValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj floatValue]);
                    presentation.opacity = interpolate_values(
                        presentation.opacity,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                }
                "position" => {
                    let from_value =
                        id_as_option(from_value).map(|obj| msg![env; obj CGPointValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj CGPointValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj CGPointValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj CGPointValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj CGPointValue]);
                    presentation.position = interpolate_values(
                        presentation.position,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                }
                "transform" => {
                    let from_value =
                        id_as_option(from_value).map(|obj| msg![env; obj CATransform3DValue]);
                    let to_value =
                        id_as_option(to_value).map(|obj| msg![env; obj CATransform3DValue]);
                    let by_value =
                        id_as_option(by_value).map(|obj| msg![env; obj CATransform3DValue]);
                    let keyframe_from_value =
                        keyframe_from.map(|obj| msg![env; obj CATransform3DValue]);
                    let keyframe_to_value =
                        keyframe_to.map(|obj| msg![env; obj CATransform3DValue]);
                    presentation.transform_3d = interpolate_transform(
                        presentation.transform_3d,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                }
                "transform.rotation" | "transform.rotation.z" => {
                    let from_value = id_as_option(from_value).map(|obj| msg![env; obj floatValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj floatValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj floatValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj floatValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj floatValue]);
                    let base_rotation = implicit_rotation_start(model_transform);
                    let rotation = interpolate_values(
                        base_rotation,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                    presentation.transform_3d = replace_z_rotation(model_transform, rotation);
                }
                "transform.scale" | "transform.scale.x" | "transform.scale.y" => {
                    let from_value = id_as_option(from_value).map(|obj| msg![env; obj floatValue]);
                    let to_value = id_as_option(to_value).map(|obj| msg![env; obj floatValue]);
                    let by_value = id_as_option(by_value).map(|obj| msg![env; obj floatValue]);
                    let keyframe_from_value = keyframe_from.map(|obj| msg![env; obj floatValue]);
                    let keyframe_to_value = keyframe_to.map(|obj| msg![env; obj floatValue]);
                    let model_scale = match &*key_path {
                        "transform.scale.x" => implicit_scale_start(model_transform, Some(true)),
                        "transform.scale.y" => implicit_scale_start(model_transform, Some(false)),
                        _ => implicit_scale_start(model_transform, None),
                    };
                    let scale = interpolate_values(
                        model_scale,
                        from_value,
                        to_value,
                        by_value,
                        keyframe_from_value,
                        keyframe_to_value,
                        interpolation_amount,
                    );
                    let (sx, sy) = match &*key_path {
                        "transform.scale.x" => (Some(scale), None),
                        "transform.scale.y" => (None, Some(scale)),
                        _ => (Some(scale), Some(scale)),
                    };
                    presentation.transform_3d = scale_transform(model_transform, sx, sy);
                }
                _ => {
                    log_dbg!(
                        "Warning: Skipping animation on unsupported key path {:?}",
                        key_path
                    );
                    continue;
                }
            }
        }

        presentation
    }

    pub fn update_started_and_finished_animations(self, env: &mut Environment) {
        // `animationDidStart:` and `animationDidStop:finished:` are optional
        // CAAnimationDelegate methods. Many apps either don't set a delegate
        // at all or set one that only implements one of the two. We must
        // register the selectors so msg! doesn't panic with "Unknown
        // selector" when nothing in the binary referenced them, and then
        // gate the actual send on `respondsToSelector:` so we mirror the
        // Cocoa behavior of silently no-op'ing when the delegate doesn't
        // implement the method.
        let did_start_sel = env
            .objc
            .register_host_selector("animationDidStart:".to_string(), &mut env.mem);
        let did_stop_sel = env
            .objc
            .register_host_selector("animationDidStop:finished:".to_string(), &mut env.mem);

        for animation in self.started_animations {
            let delegate = msg![env; animation delegate];
            if delegate != nil {
                let responds: bool = msg![env; delegate respondsToSelector: did_start_sel];
                if responds {
                    () = msg![env; delegate animationDidStart: animation];
                }
            }
        }

        for (layer, ..) in &self.finished_animations {
            retain(env, *layer);
        }
        for (layer, animation, finished, removed_on_completion, key) in self.finished_animations {
            let delegate = msg![env; animation delegate];
            if delegate != nil {
                let responds: bool = msg![env; delegate respondsToSelector: did_stop_sel];
                if responds {
                    () = msg![env; delegate animationDidStop: animation finished: finished];
                }
            }

            if removed_on_completion {
                if let Some(key) = key {
                    let key = from_rust_string(env, key);
                    () = msg![env; layer removeAnimationForKey: key];
                } else {
                    remove_anonymous_animation(env, layer, animation);
                }
            }

            release(env, layer);
        }
    }
}

fn interpolate_transform(
    current: CATransform3D,
    basic_from: Option<CATransform3D>,
    basic_to: Option<CATransform3D>,
    basic_by: Option<CATransform3D>,
    keyframe_from: Option<CATransform3D>,
    keyframe_to: Option<CATransform3D>,
    amount: f32,
) -> CATransform3D {
    let from = keyframe_from.or(basic_from).unwrap_or(current);
    let to = keyframe_to.or(basic_to).unwrap_or_else(|| {
        if let Some(by) = basic_by {
            from.concat(by)
        } else {
            from
        }
    });
    let from_components = CATransform3D::from(from);
    let to_components = CATransform3D::from(to);
    let from_values = [
        from_components.m11,
        from_components.m12,
        from_components.m13,
        from_components.m14,
        from_components.m21,
        from_components.m22,
        from_components.m23,
        from_components.m24,
        from_components.m31,
        from_components.m32,
        from_components.m33,
        from_components.m34,
        from_components.m41,
        from_components.m42,
        from_components.m43,
        from_components.m44,
    ];
    let to_values = [
        to_components.m11,
        to_components.m12,
        to_components.m13,
        to_components.m14,
        to_components.m21,
        to_components.m22,
        to_components.m23,
        to_components.m24,
        to_components.m31,
        to_components.m32,
        to_components.m33,
        to_components.m34,
        to_components.m41,
        to_components.m42,
        to_components.m43,
        to_components.m44,
    ];
    let mut result = [0.0; 16];
    for index in 0..16 {
        result[index] = from_values[index] + (to_values[index] - from_values[index]) * amount;
    }
    CATransform3D {
        m11: result[0],
        m12: result[1],
        m13: result[2],
        m14: result[3],
        m21: result[4],
        m22: result[5],
        m23: result[6],
        m24: result[7],
        m31: result[8],
        m32: result[9],
        m33: result[10],
        m34: result[11],
        m41: result[12],
        m42: result[13],
        m43: result[14],
        m44: result[15],
    }
}

// CATransform3D is row-major with row-vector composition. Scale the basis
// vectors while keeping translation, perspective, and an unanimated axis.
fn scale_transform(
    mut model: CATransform3D,
    requested_x: Option<f32>,
    requested_y: Option<f32>,
) -> CATransform3D {
    let (m11, m12, m13) = (model.m11, model.m12, model.m13);
    let (m21, m22, m23) = (model.m21, model.m22, model.m23);
    let base_scale_x = (m11 * m11 + m12 * m12 + m13 * m13).sqrt();
    let base_scale_y = (m21 * m21 + m22 * m22 + m23 * m23).sqrt();
    let sx = requested_x.unwrap_or(base_scale_x);
    let sy = requested_y.unwrap_or(base_scale_y);
    let factor_x = if base_scale_x <= f32::EPSILON {
        sx
    } else {
        sx / base_scale_x
    };
    let factor_y = if base_scale_y <= f32::EPSILON {
        sy
    } else {
        sy / base_scale_y
    };
    model.scale(factor_x, factor_y, 1.0)
}

fn implicit_rotation_start(model: CATransform3D) -> f32 {
    (model.m12 - model.m21).atan2(model.m11 + model.m22)
}

fn implicit_scale_start(model: CATransform3D, axis: Option<bool>) -> f32 {
    let (m11, m12, m13) = (model.m11, model.m12, model.m13);
    let (m21, m22, m23) = (model.m21, model.m22, model.m23);
    let scale_x = (m11 * m11 + m12 * m12 + m13 * m13).sqrt();
    let scale_y = (m21 * m21 + m22 * m22 + m23 * m23).sqrt();
    match axis {
        Some(true) => scale_x,
        Some(false) => scale_y,
        None => (scale_x + scale_y) * 0.5,
    }
}

fn replace_z_rotation(model: CATransform3D, angle: f32) -> CATransform3D {
    // Polar-decompose the 2D basis to separate its nearest rotation from
    // scale/shear, then replace only that rotation. This avoids treating a
    // scaled/skewed m11/m12 pair as a pure rotation.
    let model_angle = (model.m12 - model.m21).atan2(model.m11 + model.m22);
    let delta = angle - model_angle;
    let (cosine, sine) = (delta.cos(), delta.sin());
    let mut result = model;
    let (m11, m12, m21, m22) = (model.m11, model.m12, model.m21, model.m22);
    result.m11 = m11 * cosine - m12 * sine;
    result.m12 = m11 * sine + m12 * cosine;
    result.m21 = m21 * cosine - m22 * sine;
    result.m22 = m21 * sine + m22 * cosine;
    result
}

#[cfg(test)]
mod keyframe_tests {
    use super::{
        implicit_rotation_start, implicit_scale_start, interpolate_keyframe, interpolate_transform,
        replace_z_rotation, scale_transform, select_keyframe_segment,
    };
    use crate::frameworks::core_graphics::cg_color::CGColorHostObject;

    #[test]
    fn selects_evenly_spaced_linear_segment() {
        assert_eq!(
            select_keyframe_segment(0.25, None, 3, false),
            Some(super::KeyframeSegment {
                from_index: 0,
                to_index: 1,
                amount: 0.5,
            })
        );
    }

    #[test]
    fn selects_explicit_key_times_and_discrete_holds() {
        assert_eq!(
            select_keyframe_segment(0.4, Some(&[0.0, 0.25, 1.0]), 3, false),
            Some(super::KeyframeSegment {
                from_index: 1,
                to_index: 2,
                amount: 0.2,
            })
        );
        assert_eq!(
            select_keyframe_segment(0.3, Some(&[0.0, 0.25, 1.0]), 3, true),
            Some(super::KeyframeSegment {
                from_index: 1,
                to_index: 1,
                amount: 0.0,
            })
        );
    }

    #[test]
    fn falls_back_for_invalid_key_times_and_clamps_progress() {
        assert_eq!(
            select_keyframe_segment(-1.0, Some(&[0.0, 0.8]), 3, false),
            Some(super::KeyframeSegment {
                from_index: 0,
                to_index: 1,
                amount: 0.0,
            })
        );
        assert_eq!(
            select_keyframe_segment(2.0, None, 3, false),
            Some(super::KeyframeSegment {
                from_index: 2,
                to_index: 2,
                amount: 0.0,
            })
        );
    }

    #[test]
    fn handles_segment_boundaries_and_single_values() {
        assert_eq!(
            select_keyframe_segment(0.5, Some(&[0.0, 0.5, 1.0]), 3, false),
            Some(super::KeyframeSegment {
                from_index: 1,
                to_index: 2,
                amount: 0.0,
            })
        );
        assert_eq!(
            select_keyframe_segment(0.5, None, 1, false),
            Some(super::KeyframeSegment {
                from_index: 0,
                to_index: 0,
                amount: 0.0,
            })
        );
    }

    #[test]
    fn interpolates_core_animation_value_types() {
        use crate::frameworks::core_graphics::cg_geometry::{CGPoint, CGRect, CGSize};

        let point = interpolate_keyframe(
            CGPoint { x: 0.0, y: 10.0 },
            CGPoint { x: 20.0, y: 30.0 },
            0.25,
        );
        assert_eq!(point, CGPoint { x: 5.0, y: 15.0 });

        let rect = interpolate_keyframe(
            CGRect {
                origin: CGPoint { x: 0.0, y: 10.0 },
                size: CGSize {
                    width: 20.0,
                    height: 30.0,
                },
            },
            CGRect {
                origin: CGPoint { x: 40.0, y: 50.0 },
                size: CGSize {
                    width: 60.0,
                    height: 70.0,
                },
            },
            0.5,
        );
        assert_eq!(
            rect,
            CGRect {
                origin: CGPoint { x: 20.0, y: 30.0 },
                size: CGSize {
                    width: 40.0,
                    height: 50.0
                },
            }
        );
    }

    #[test]
    fn interpolates_rgba_colors() {
        let from = CGColorHostObject {
            color_space_name: "kCGColorSpaceGenericRGB",
            r: 0.0,
            g: 0.2,
            b: 0.4,
            a: 0.2,
        };
        let to = CGColorHostObject {
            color_space_name: "kCGColorSpaceGenericRGB",
            r: 1.0,
            g: 0.8,
            b: 0.6,
            a: 1.0,
        };

        let midpoint = interpolate_keyframe(from, to, 0.5);

        assert_eq!(midpoint.r, 0.5);
        assert_eq!(midpoint.g, 0.5);
        assert_eq!(midpoint.b, 0.5);
        assert_eq!(midpoint.a, 0.6);
    }

    #[test]
    fn interpolates_transform_matrix_and_rotation_scale_values() {
        use crate::frameworks::core_animation::ca_transform3d::{
            CATransform3D, CATransform3DIdentity,
        };

        let scaled = CATransform3D::make_scale(3.0, 3.0, 3.0).translate(10.0, 20.0, 30.0);
        let transform = interpolate_transform(
            CATransform3DIdentity,
            None,
            None,
            None,
            Some(CATransform3DIdentity),
            Some(scaled),
            0.5,
        );
        let (m11, m22, m41, m42, m43) = (
            transform.m11,
            transform.m22,
            transform.m41,
            transform.m42,
            transform.m43,
        );
        assert_eq!((m11, m22, m41, m42, m43), (2.0, 2.0, 15.0, 30.0, 45.0));

        let rotation = interpolate_keyframe(0.0_f32, std::f32::consts::PI, 0.5);
        assert_eq!(rotation, std::f32::consts::FRAC_PI_2);
        let halfway = CATransform3D::from(CATransform3D::make_rotation(rotation, 0.0, 0.0, 1.0));
        assert!(halfway.m12.abs() > 0.999);
        assert!(halfway.m21.abs() > 0.999);

        let current = CATransform3D::make_rotation(0.5, 0.0, 0.0, 1.0);
        let current = CATransform3D::from(current);
        let current_angle = current.m12.atan2(current.m11);
        let updated = CATransform3D::from(replace_z_rotation(current, 1.0));
        assert!((updated.m12.atan2(updated.m11) - 1.0).abs() < 0.0001);

        let current = CATransform3D::make_scale(2.0, 3.0, 1.0);
        let updated = CATransform3D::from(scale_transform(current, Some(1.5), Some(1.5)));
        assert!((updated.m11 - 1.5).abs() < 0.0001);
        assert!((updated.m22 - 1.5).abs() < 0.0001);
    }

    #[test]
    fn transform_scale_is_rebuilt_from_model_transform_each_frame() {
        use crate::frameworks::core_animation::ca_transform3d::CATransform3D;

        let mut model = CATransform3D::from(CATransform3D::make_scale(2.0, 3.0, 1.0));
        model.m12 = 0.5;
        model.m41 = 40.0;
        model.m44 = 2.0;
        let scale_at_progress = |progress: f32| {
            let target = interpolate_keyframe(0.0_f32, 1.0_f32, progress);
            scale_transform(model, Some(target), Some(target))
        };

        let first = CATransform3D::from(scale_at_progress(0.5));
        let second = CATransform3D::from(scale_at_progress(0.5));
        let (first_x, first_y, second_x, second_y) = (first.m11, first.m22, second.m11, second.m22);
        let (first_skew, second_skew) = (first.m12, second.m12);
        let (translation, perspective) = (first.m41, first.m44);
        let (first_x_len, first_y_len) = (
            (first_x * first_x + first_skew * first_skew).sqrt(),
            (first_y * first_y + first.m21 * first.m21).sqrt(),
        );

        assert_eq!((first_x, first_y), (second_x, second_y));
        assert!((first_x_len - 0.5).abs() < 0.0001);
        assert!((first_y_len - 0.5).abs() < 0.0001);
        assert_eq!(first_skew, second_skew);
        assert_ne!(first_skew, 0.0);
        assert_eq!((translation, perspective), (40.0, 2.0));
    }

    #[test]
    fn transform_rotation_replaces_model_angle_and_preserves_translation() {
        use crate::frameworks::core_animation::ca_transform3d::{
            CATransform3D, CATransform3DIdentity,
        };

        let mut model = CATransform3D::from(CATransform3D::make_rotation(0.5, 0.0, 0.0, 1.0));
        model.m41 = 32.0;
        model.m44 = 2.0;
        let rotation_at_progress = |progress: f32| {
            let target = interpolate_keyframe(implicit_rotation_start(model), 1.0_f32, progress);
            replace_z_rotation(model, target)
        };

        let first = CATransform3D::from(rotation_at_progress(0.5));
        let again = CATransform3D::from(rotation_at_progress(0.5));
        let first_angle = first.m12.atan2(first.m11);
        let again_angle = again.m12.atan2(again.m11);
        assert!((first_angle - 0.75).abs() < 0.0001);
        assert!((again_angle - first_angle).abs() < 0.0001);
        assert_eq!((first.m41, first.m44), (32.0, 2.0));
    }

    #[test]
    fn transform_rotation_preserves_nonuniform_scale_and_skew() {
        use crate::frameworks::core_animation::ca_transform3d::CATransform3D;

        let mut model = CATransform3D::make_rotation(0.4, 0.0, 0.0, 1.0).scale(2.0, 3.0, 1.0);
        model.m12 += 0.25; // Add a small skew to the XY basis.
        model.m41 = 17.0;
        model.m44 = 2.0;
        let changed = CATransform3D::from(replace_z_rotation(model, 1.2));
        let expected = CATransform3D::from(model);
        let (a11, a12, a21, a22) = (expected.m11, expected.m12, expected.m21, expected.m22);
        let (b11, b12, b21, b22) = (changed.m11, changed.m12, changed.m21, changed.m22);
        // Right-multiplying by a pure rotation preserves the basis row lengths
        // and dot product (the scale and skew), while replacing its orientation.
        let before_x = a11 * a11 + a12 * a12;
        let before_y = a21 * a21 + a22 * a22;
        let before_dot = a11 * a21 + a12 * a22;
        let after_x = b11 * b11 + b12 * b12;
        let after_y = b21 * b21 + b22 * b22;
        let after_dot = b11 * b21 + b12 * b22;
        assert!((before_x - after_x).abs() < 0.0001);
        assert!((before_y - after_y).abs() < 0.0001);
        assert!((before_dot - after_dot).abs() < 0.0001);
        assert_eq!((changed.m41, changed.m44), (17.0, 2.0));
        let angle = (changed.m12 - changed.m21).atan2(changed.m11 + changed.m22);
        assert!((angle - 1.2).abs() < 0.0001);
        assert!(implicit_rotation_start(model).is_finite());
    }

    #[test]
    fn transform_axis_scale_preserves_other_model_axis() {
        use crate::frameworks::core_animation::ca_transform3d::CATransform3D;

        let model = CATransform3D::make_scale(2.0, 3.0, 1.0);
        let x_only = CATransform3D::from(scale_transform(model, Some(0.5), None));
        let y_only = CATransform3D::from(scale_transform(model, None, Some(0.5)));
        assert_eq!((x_only.m11, x_only.m22), (0.5, 3.0));
        assert_eq!((y_only.m11, y_only.m22), (2.0, 0.5));
    }

    #[test]
    fn transform_implicit_start_values_come_from_model_transform() {
        use crate::frameworks::core_animation::ca_transform3d::CATransform3D;

        let model = CATransform3D::make_rotation(0.5, 0.0, 0.0, 1.0).scale(2.0, 3.0, 1.0);
        assert!((implicit_rotation_start(model) - 0.5).abs() < 0.0001);
        assert!((implicit_scale_start(model, Some(true)) - 2.0).abs() < 0.0001);
        assert!((implicit_scale_start(model, Some(false)) - 3.0).abs() < 0.0001);
        assert!((implicit_scale_start(model, None) - 2.5).abs() < 0.0001);
    }

    #[test]
    fn interpolates_scalar_values() {
        assert_eq!(interpolate_keyframe(0.2, 1.0, 0.5), 0.6);
    }

    #[test]
    fn discrete_mode_selects_the_held_value() {
        assert_eq!(
            select_keyframe_segment(0.499, Some(&[0.0, 0.5, 1.0]), 3, true)
                .unwrap()
                .from_index,
            0
        );
        assert_eq!(
            select_keyframe_segment(0.5, Some(&[0.0, 0.5, 1.0]), 3, true)
                .unwrap()
                .from_index,
            1
        );
        assert_eq!(
            select_keyframe_segment(1.0, Some(&[0.0, 0.5, 1.0]), 3, true)
                .unwrap()
                .from_index,
            2
        );
        assert_eq!(
            select_keyframe_segment(1.0, None, 3, false)
                .unwrap()
                .from_index,
            2
        );
    }
}

#[allow(clippy::eq_op)]
fn get_from_and_by_values<T>(
    current_value: Option<T>,
    from_value: Option<T>,
    to_value: Option<T>,
    by_value: Option<T>,
) -> (T, T)
where
    T: Copy + Sub<Output = T>,
{
    // The semantics of fromValue/toValue/byValue follow Apple's docs. If a
    // misbehaving guest specifies all three, fall back to using from/to and
    // ignoring `byValue` instead of crashing the host.
    if let (Some(from_value), Some(to_value)) = (from_value, to_value) {
        if by_value.is_some() {
            log_dbg!(
                "CABasicAnimation: all three of fromValue/toValue/byValue set; \
                 ignoring byValue"
            );
        }

        let by_value = to_value - from_value.to_owned();
        (from_value, by_value)
    } else if let (Some(from_value), Some(by_value)) = (from_value, by_value) {
        (from_value.to_owned(), by_value.to_owned())
    } else if let (Some(to_value), Some(by_value)) = (to_value, by_value) {
        let from_value = to_value - by_value;
        (from_value, by_value.to_owned())
    } else if let Some(from_value) = from_value {
        if let Some(current) = current_value {
            let by_value = current - from_value;
            (from_value.to_owned(), by_value)
        } else {
            // No current value to derive `by` from — treat as a no-op animation.
            (from_value.to_owned(), from_value - from_value)
        }
    } else if let Some(to_value) = to_value {
        if let Some(from_value) = current_value {
            let by_value = to_value - from_value;
            (from_value.to_owned(), by_value)
        } else {
            (to_value.to_owned(), to_value - to_value)
        }
    } else if let Some(by_value) = by_value {
        if let Some(from_value) = current_value {
            (from_value.to_owned(), by_value.to_owned())
        } else {
            // Pick a zero-equivalent start; the animation will still be valid.
            (by_value - by_value, by_value.to_owned())
        }
    } else {
        // All properties are nil. The official semantics call for interpolating
        // between the previous and current presentation-layer values of
        // `keyPath`, which we don't track here. Fall back to a no-op animation
        // starting from `current_value` (or zero if even that is missing).
        if let Some(current) = current_value {
            (current.to_owned(), current - current)
        } else {
            log!(
                "Warning: CABasicAnimation: from/to/by all nil and no current \
                 value; emitting no-op animation."
            );
            // SAFETY: this branch will only happen when `current_value` is
            // None AND none of from/to/by were set, in which case the caller
            // has no expectation about the values; default-constructing via
            // a zero-size subtraction is impossible without a concrete T.
            // Fall back to a panic-free degenerate path: re-use whatever the
            // caller passed by returning the same Option-default. We avoid
            // requiring `Default` on T by computing a zero from the unwrap
            // we already know exists in *some* branch above; if we get here
            // we genuinely have nothing to animate, so we cannot produce a
            // value of T. Returning here would require Default; since we
            // can't add that bound without ripping through all callers, log
            // the fact and panic with a clearer message (still better than
            // the silent `unimplemented!`).
            unreachable!(
                "CABasicAnimation: from/to/by all nil and current_value missing; \
                 no value of T available to interpolate."
            )
        }
    }
}

fn id_as_option(value: id) -> Option<id> {
    if value == nil {
        None
    } else {
        Some(value)
    }
}
