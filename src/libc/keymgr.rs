/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `keymgr.h` (KeyMgr).
//!
//! KeyMgr's only documentation seems to be in its [public source code](https://github.com/apple-opensource/keymgr).

use crate::dyld::{export_c_func, FunctionExports};
use crate::environment::ThreadBlock;
use crate::mem::{MutPtr, MutVoidPtr, Ptr};
use crate::{Environment, ThreadId};
use std::collections::hash_map::{Entry, HashMap};

#[derive(Default)]
pub struct State {
    // (process-wide pointer, owning guest thread, recursive acquisition depth)
    processwide_ptrs: HashMap<i32, (MutVoidPtr, Option<ThreadId>, usize)>,
}

fn get_and_lock_processwide_ptr_inner(env: &mut Environment, key: i32) -> Result<MutVoidPtr, i32> {
    loop {
        let locked_by_another_thread = match env.libc_state.keymgr.processwide_ptrs.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert((Ptr::null(), Some(env.current_thread), 1));
                return Ok(Ptr::null());
            }
            Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                match state.1 {
                    None => {
                        state.1 = Some(env.current_thread);
                        state.2 = 1;
                        return Ok(state.0);
                    }
                    Some(owner) if owner == env.current_thread => {
                        // KeyMgr locks are re-entrant for their owning guest
                        // thread. Keep the depth so an inner unlock does not
                        // release the outer caller's lock.
                        state.2 = state.2.saturating_add(1);
                        return Ok(state.0);
                    }
                    Some(_) => true,
                }
            }
        };

        if locked_by_another_thread {
            // Guest execution is cooperative: yield so the current owner can
            // finish its KeyMgr operation, then retry after the scheduler
            // observes the lock as free.
            env.yield_thread(ThreadBlock::KeymgrLock(key));
        }
    }
}

fn _keymgr_get_and_lock_processwide_ptr_2(
    env: &mut Environment,
    key: i32,
    result: MutPtr<MutVoidPtr>,
) -> i32 {
    match get_and_lock_processwide_ptr_inner(env, key) {
        Ok(ptr) => {
            env.mem.write(result, ptr);
            0 // success
        }
        Err(err) => err,
    }
}

fn _keymgr_get_and_lock_processwide_ptr(env: &mut Environment, key: i32) -> MutVoidPtr {
    get_and_lock_processwide_ptr_inner(env, key).unwrap_or_default()
}

fn _keymgr_set_and_unlock_processwide_ptr(env: &mut Environment, key: i32, ptr: MutVoidPtr) -> i32 {
    let entry = env
        .libc_state
        .keymgr
        .processwide_ptrs
        .get_mut(&key)
        .unwrap();
    assert_eq!(entry.1, Some(env.current_thread));
    entry.0 = ptr;
    if entry.2 > 1 {
        entry.2 -= 1;
    } else {
        entry.1 = None;
        entry.2 = 0;
    }
    0 // success
}

fn _keymgr_unlock_processwide_ptr(env: &mut Environment, key: i32) -> i32 {
    let entry = env
        .libc_state
        .keymgr
        .processwide_ptrs
        .get_mut(&key)
        .unwrap();
    assert_eq!(entry.1, Some(env.current_thread));
    if entry.2 > 1 {
        entry.2 -= 1;
    } else {
        entry.1 = None;
        entry.2 = 0;
    }
    0 // success
}

impl State {
    /// Whether a waiting guest can retry acquiring a process-wide pointer.
    pub(crate) fn processwide_ptr_is_unlocked(&self, key: i32) -> bool {
        self.processwide_ptrs
            .get(&key)
            .is_none_or(|(_, owner, _)| owner.is_none())
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(_keymgr_get_and_lock_processwide_ptr_2(_, _)),
    export_c_func!(_keymgr_get_and_lock_processwide_ptr(_)),
    export_c_func!(_keymgr_set_and_unlock_processwide_ptr(_, _)),
    export_c_func!(_keymgr_unlock_processwide_ptr(_)),
];
