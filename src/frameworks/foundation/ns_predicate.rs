/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSPredicate`.

use crate::frameworks::foundation::ns_string;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - Predicate kind
// =========================================================================

enum PredicateKind {
    /// Always evaluates to the given constant. Also acts as the
    /// `Default` for the phantom-fallback host object (yielding a
    /// predicate that always returns `false`).
    Constant(bool),
    /// Format predicate, parsed when evaluated.
    Format,
    /// NOT of another predicate.
    Not(id),
    /// AND of two predicates.
    And(id, id),
    /// OR of two predicates.
    Or(id, id),
    /// Block-based predicate (not supported — always returns false).
    Block,
}
impl Default for PredicateKind {
    fn default() -> Self {
        PredicateKind::Constant(false)
    }
}

#[derive(Default)]
struct NSPredicateHostObject {
    kind: PredicateKind,
    /// Original format string if created with predicateWithFormat:.
    format: Option<String>,
    /// Retained values supplied as positional format arguments.
    arguments: Vec<id>,
    /// Retained values supplied as named variable substitutions.
    variables: std::collections::HashMap<String, id>,
}
impl HostObject for NSPredicateHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSPredicate: NSObject

// =========================================================================
// MARK: - Allocation
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSPredicateHostObject {
        kind: PredicateKind::Constant(false),
        format: None,
        arguments: Vec::new(),
        variables: std::collections::HashMap::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// =========================================================================
// MARK: - Convenience constructors
// =========================================================================

// predicateWithValue: — always-true or always-false predicate.
+ (id)predicateWithValue:(bool)value {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithValue:value];
    crate::objc::autorelease(env, new)
}

// predicateWithFormat: — stores the format string for shared evaluation.
+ (id)predicateWithFormat:(id)format { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithFormat:format];
    crate::objc::autorelease(env, new)
}

// predicateWithFormat:argumentArray:
+ (id)predicateWithFormat:(id)format argumentArray:(id)_arguments {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithFormat:format argumentArray:_arguments];
    crate::objc::autorelease(env, new)
}

// predicateWithBlock: — block evaluation is not supported; the predicate
// always evaluates to false (and logs when evaluated).
+ (id)predicateWithBlock:(id)_block {
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<NSPredicateHostObject>(new).kind = PredicateKind::Block;
    crate::objc::autorelease(env, new)
}

// notPredicateWithSubpredicate:
+ (id)notPredicateWithSubpredicate:(id)predicate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithNotPredicate:predicate];
    crate::objc::autorelease(env, new)
}

// andPredicateWithSubpredicates:
+ (id)andPredicateWithSubpredicates:(id)subpredicates { // NSArray*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithAndPredicates:subpredicates];
    crate::objc::autorelease(env, new)
}

// orPredicateWithSubpredicates:
+ (id)orPredicateWithSubpredicates:(id)subpredicates { // NSArray*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithOrPredicates:subpredicates];
    crate::objc::autorelease(env, new)
}

// =========================================================================
// MARK: - Initializers
// =========================================================================

- (id)initWithValue:(bool)value {
    env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
        PredicateKind::Constant(value);
    this
}

- (id)initWithFormat:(id)format { // NSString*
    let fmt_str = if format != nil {
        ns_string::to_rust_string(env, format).into_owned()
    } else {
        String::new()
    };
    log_dbg!("NSPredicate initWithFormat: {:?}", fmt_str);
    {
        let host = env.objc.borrow_mut::<NSPredicateHostObject>(this);
        host.format = Some(fmt_str.clone());
        host.kind = PredicateKind::Format;
    }
    this
}

- (id)initWithFormat:(id)format argumentArray:(id)_arguments {
    let new: id = msg![env; this initWithFormat:format];
    if new == nil || _arguments == nil {
        return new;
    }
    let count: crate::frameworks::foundation::NSUInteger = msg![env; _arguments count];
    let mut arguments = Vec::with_capacity(count as usize);
    for index in 0..count {
        let argument: id = msg![env; _arguments objectAtIndex:index];
        retain(env, argument);
        arguments.push(argument);
    }
    env.objc.borrow_mut::<NSPredicateHostObject>(new).arguments = arguments;
    new
}

- (id)initWithNotPredicate:(id)subpredicate {
    retain(env, subpredicate);
    env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
        PredicateKind::Not(subpredicate);
    this
}

- (id)initWithAndPredicates:(id)subpredicates { // NSArray*
    // For simplicity store as AND of first two; a full implementation would
    // fold the entire array. Most callers pass exactly two predicates.
    let count: crate::frameworks::foundation::NSUInteger =
        msg![env; subpredicates count];
    if count == 0 {
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::Constant(true); // vacuously true
    } else if count == 1 {
        // A single-element AND evaluates the same as its only subpredicate.
        // Store it as And(p, p): p && p == p, and dealloc releases both
        // retains.
        let p: id = msg![env; subpredicates objectAtIndex:0u32];
        retain(env, p);
        retain(env, p);
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::And(p, p);
    } else {
        let p0: id = msg![env; subpredicates objectAtIndex:0u32];
        let p1: id = msg![env; subpredicates objectAtIndex:1u32];
        retain(env, p0);
        retain(env, p1);
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::And(p0, p1);
    }
    this
}

- (id)initWithOrPredicates:(id)subpredicates { // NSArray*
    let count: crate::frameworks::foundation::NSUInteger =
        msg![env; subpredicates count];
    if count == 0 {
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::Constant(false); // vacuously false
    } else if count == 1 {
        // A single-element OR evaluates the same as its only subpredicate.
        let p: id = msg![env; subpredicates objectAtIndex:0u32];
        retain(env, p);
        retain(env, p);
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::Or(p, p);
    } else {
        let p0: id = msg![env; subpredicates objectAtIndex:0u32];
        let p1: id = msg![env; subpredicates objectAtIndex:1u32];
        retain(env, p0);
        retain(env, p1);
        env.objc.borrow_mut::<NSPredicateHostObject>(this).kind =
            PredicateKind::Or(p0, p1);
    }
    this
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let kind = std::mem::replace(
        &mut env.objc.borrow_mut::<NSPredicateHostObject>(this).kind,
        PredicateKind::Constant(false),
    );
    match kind {
        PredicateKind::Not(p) => release(env, p),
        PredicateKind::And(a, b) | PredicateKind::Or(a, b) => {
            release(env, a);
            release(env, b);
        }
        _ => {}
    }
    let arguments = std::mem::take(&mut env.objc.borrow_mut::<NSPredicateHostObject>(this).arguments);
    for argument in arguments {
        release(env, argument);
    }
    let variables = std::mem::take(&mut env.objc.borrow_mut::<NSPredicateHostObject>(this).variables);
    for value in variables.into_values() {
        release(env, value);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - NSCopying
// =========================================================================

- (id)copyWithZone:(NSZonePtr)_zone {
    let (kind, format, arguments, variables) = {
        let host = env.objc.borrow::<NSPredicateHostObject>(this);
        let kind = match &host.kind {
            PredicateKind::Constant(value) => PredicateKind::Constant(*value),
            PredicateKind::Format => PredicateKind::Format,
            PredicateKind::Not(predicate) => PredicateKind::Not(*predicate),
            PredicateKind::And(left, right) => PredicateKind::And(*left, *right),
            PredicateKind::Or(left, right) => PredicateKind::Or(*left, *right),
            PredicateKind::Block => PredicateKind::Block,
        };
        (
            kind,
            host.format.clone(),
            host.arguments.clone(),
            host.variables.clone(),
        )
    };
    let copy: id = msg![env; this alloc];
    for predicate in match &kind {
        PredicateKind::Not(predicate) => vec![*predicate],
        PredicateKind::And(left, right) | PredicateKind::Or(left, right) => vec![*left, *right],
        _ => Vec::new(),
    } {
        retain(env, predicate);
    }
    for argument in &arguments {
        retain(env, *argument);
    }
    for value in variables.values() {
        retain(env, *value);
    }
    *env.objc.borrow_mut::<NSPredicateHostObject>(copy) = NSPredicateHostObject {
        kind,
        format,
        arguments,
        variables,
    };
    copy
}

// =========================================================================
// MARK: - Evaluation
// =========================================================================

// evaluateWithObject: — evaluate against a single object.
- (bool)evaluateWithObject:(id)object {
    evaluate_predicate(env, this, object, &[])
}

// evaluateWithObject:substitutionVariables: — resolve basic variables.
- (bool)evaluateWithObject:(id)object
   substitutionVariables:(id)_variables { // NSDictionary*
    let mut variables = Vec::new();
    if _variables != nil {
        let keys: id = msg![env; _variables allKeys];
        let count: crate::frameworks::foundation::NSUInteger = msg![env; keys count];
        for index in 0..count {
            let key: id = msg![env; keys objectAtIndex:index];
            let name = substitution_variable_name(ns_string::to_rust_string(env, key).as_ref());
            let value: id = msg![env; _variables objectForKey:key];
            variables.push((name, value));
        }
    }
    evaluate_predicate(env, this, object, &variables)
}

// =========================================================================
// MARK: - Predicate string / subpredicates
// =========================================================================

- (id)predicateFormat { // NSString*
    let fmt = {
        let host = env.objc.borrow::<NSPredicateHostObject>(this);
        match &host.kind {
            PredicateKind::Constant(v) => {
                if *v { "TRUEPREDICATE".to_string() } else { "FALSEPREDICATE".to_string() }
            }
            PredicateKind::Format => host.format.clone().unwrap_or_default(),
            PredicateKind::Not(_)    => "NOT (...)".to_string(),
            PredicateKind::And(_, _) => "(...) AND (...)".to_string(),
            PredicateKind::Or(_, _)  => "(...) OR (...)".to_string(),
            PredicateKind::Block     => "BLOCK".to_string(),
        }
    };
    let s: id = ns_string::from_rust_string(env, fmt);
    crate::objc::autorelease(env, s)
}

// Return a copied predicate; variable operands are resolved on evaluation.
- (id)predicateWithSubstitutionVariables:(id)_variables {
    let new: id = msg![env; this copy];
    if _variables != nil {
        let keys: id = msg![env; _variables allKeys];
        let count: crate::frameworks::foundation::NSUInteger = msg![env; keys count];
        for index in 0..count {
            let key: id = msg![env; keys objectAtIndex:index];
            let name = substitution_variable_name(ns_string::to_rust_string(env, key).as_ref());
            let value: id = msg![env; _variables objectForKey:key];
            retain(env, value);
            let old = env.objc.borrow_mut::<NSPredicateHostObject>(new).variables.insert(name, value);
            if let Some(old) = old {
                release(env, old);
            }
        }
    }
    crate::objc::autorelease(env, new)
}

// =========================================================================
// MARK: - Compound predicate subpredicates accessors
// =========================================================================

- (id)subpredicates { // NSArray*
    let (a, b) = {
        let host = env.objc.borrow::<NSPredicateHostObject>(this);
        match &host.kind {
            PredicateKind::And(a, b) | PredicateKind::Or(a, b) => (*a, *b),
            PredicateKind::Not(p) => (*p, nil),
            _ => (nil, nil),
        }
    };
    if a == nil {
        return msg_class![env; NSArray array];
    }
    let arr: id = msg_class![env; NSMutableArray new];
    let _: () = msg![env; arr addObject:a];
    if b != nil {
        let _: () = msg![env; arr addObject:b];
    }
    crate::objc::autorelease(env, arr)
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let fmt: id = msg![env; this predicateFormat];
    let fmt_str = ns_string::to_rust_string(env, fmt);
    let s = format!("<NSPredicate: {:?} \"{}\">", this, fmt_str);
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// =========================================================================
// MARK: - NSCompoundPredicate (subclass — thin wrapper)
// =========================================================================

@implementation NSCompoundPredicate: NSPredicate

+ (id)notPredicateWithSubpredicate:(id)predicate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithNotPredicate:predicate];
    crate::objc::autorelease(env, new)
}

+ (id)andPredicateWithSubpredicates:(id)subpredicates {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithAndPredicates:subpredicates];
    crate::objc::autorelease(env, new)
}

+ (id)orPredicateWithSubpredicates:(id)subpredicates {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithOrPredicates:subpredicates];
    crate::objc::autorelease(env, new)
}

@end

// =========================================================================
// MARK: - NSComparisonPredicate (subclass — stub)
// =========================================================================

@implementation NSComparisonPredicate: NSPredicate

+ (id)predicateWithLeftExpression:(id)_lhs
                  rightExpression:(id)_rhs
                         modifier:(i32)_modifier
                             type:(i32)_type
                          options:(i32)_options {
    log!("TODO: NSComparisonPredicate predicateWithLeftExpression: — returning false predicate");
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithValue:false];
    crate::objc::autorelease(env, new)
}

+ (id)predicateWithLeftExpression:(id)_lhs
                  rightExpression:(id)_rhs
               customSelector:(id)_selector {
    log!("TODO: NSComparisonPredicate predicateWithLeftExpression:customSelector: — returning false predicate");
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithValue:false];
    crate::objc::autorelease(env, new)
}

@end

};

// =========================================================================
// MARK: - Evaluation helper
// =========================================================================

fn evaluate_predicate(
    env: &mut crate::Environment,
    predicate: crate::objc::id,
    object: crate::objc::id,
    variables: &[(String, id)],
) -> bool {
    // Read kind without holding borrow across recursive calls.
    enum Snapshot {
        Format(String, Vec<id>, Vec<(String, id)>),
        Not(id),
        And(id, id),
        Or(id, id),
    }
    let snapshot = {
        let host = env.objc.borrow::<NSPredicateHostObject>(predicate);
        match &host.kind {
            PredicateKind::Constant(v) => return *v,
            PredicateKind::Block => {
                log!("NSPredicate block evaluation not supported — returning false");
                return false;
            }
            PredicateKind::Format => Snapshot::Format(
                host.format.clone().unwrap_or_default(),
                host.arguments.clone(),
                host.variables
                    .iter()
                    .map(|(name, value)| (name.clone(), *value))
                    .collect(),
            ),
            PredicateKind::Not(predicate) => Snapshot::Not(*predicate),
            PredicateKind::And(left, right) => Snapshot::And(*left, *right),
            PredicateKind::Or(left, right) => Snapshot::Or(*left, *right),
        }
    };
    match snapshot {
        Snapshot::Not(predicate) => !evaluate_predicate(env, predicate, object, variables),
        Snapshot::And(left, right) => {
            evaluate_predicate(env, left, object, variables)
                && evaluate_predicate(env, right, object, variables)
        }
        Snapshot::Or(left, right) => {
            evaluate_predicate(env, left, object, variables)
                || evaluate_predicate(env, right, object, variables)
        }
        Snapshot::Format(format, arguments, stored_variables) => {
            let expression = match super::ns_predicate_format::parse_with_arguments(&format) {
                Ok(expression) => expression,
                Err(error) => {
                    log!("NSPredicate format not supported ({:?}): {}", format, error);
                    return false;
                }
            };
            let mut variable_operands = Vec::new();
            for (name, value) in stored_variables.iter().chain(variables.iter()) {
                let value: id = *value;
                let operand = match object_to_operand(env, value) {
                    Some(operand) => operand,
                    None => {
                        log!(
                            "NSPredicate substitution variable {:?} has unsupported value",
                            name
                        );
                        return false;
                    }
                };
                variable_operands.push((name.clone(), operand));
            }
            let expression = expression.substitute(&variable_operands);
            let resolved_arguments: Option<Vec<_>> = arguments
                .iter()
                .map(|argument| {
                    let argument: id = *argument;
                    object_to_operand(env, argument)
                })
                .collect::<Option<Vec<_>>>();
            let Some(resolved_arguments) = resolved_arguments else {
                log!(
                    "NSPredicate format {:?} has unsupported argument value",
                    format
                );
                return false;
            };
            expression
                .evaluate(
                    |key_path| {
                        let key = ns_string::from_rust_string(env, key_path.to_string());
                        let value: id = msg![env; object valueForKeyPath:key];
                        release(env, key);
                        if value == nil {
                            Some(super::ns_predicate_format::Operand::Null)
                        } else {
                            object_to_operand(env, value)
                        }
                    },
                    &resolved_arguments,
                )
                .unwrap_or_else(|error| {
                    log!(
                        "NSPredicate evaluation unsupported for {:?}: {}",
                        format,
                        error
                    );
                    false
                })
        }
    }
}

fn object_to_operand(
    env: &mut crate::Environment,
    object: crate::objc::id,
) -> Option<super::ns_predicate_format::Operand> {
    use super::ns_predicate_format::Operand;
    if object == nil {
        return Some(Operand::Null);
    }
    let null_class = env.objc.get_known_class("NSNull", &mut env.mem);
    if msg![env; object isKindOfClass:null_class] {
        return Some(Operand::Null);
    }
    let number_class = env.objc.get_known_class("NSNumber", &mut env.mem);
    if msg![env; object isKindOfClass:number_class] {
        return Some(
            match *env
                .objc
                .borrow::<super::ns_value::NSNumberHostObject>(object)
            {
                super::ns_value::NSNumberHostObject::Bool(value) => Operand::Bool(value),
                _ => Operand::Number(msg![env; object doubleValue]),
            },
        );
    }
    let string_class = env.objc.get_known_class("NSString", &mut env.mem);
    if msg![env; object isKindOfClass:string_class] {
        return Some(Operand::String(
            ns_string::to_rust_string(env, object).into_owned(),
        ));
    }
    None
}

fn substitution_variable_name(name: &str) -> String {
    if name.starts_with('$') {
        name.to_owned()
    } else {
        format!("${name}")
    }
}
