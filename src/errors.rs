//! This module keeps track of errors that have occurred "in the background",
//! i.e. not as a direct, immediate result of user action. In particular this
//! would include audio API errors and errors during a metadata scan.
//!
//! All errors should *also* be logged to stderr. This module doesn't handle
//! that. All it does is marshal "relevant" errors for display in the UI.

use crate::*;

use lazy_static::lazy_static;

use std::{
    collections::BTreeMap,
    sync::{RwLock, RwLockReadGuard},
};

static GENERATION: GenerationTracker = GenerationTracker::new();

lazy_static! {
    static ref ERRORS
        : RwLock<BTreeMap<String,Vec<String>>>
        = RwLock::new(BTreeMap::new());
}

// Both of these could be rewritten to use Entry, but it's not that important
// to do so.

pub fn reset_from(whom: &str) {
    let mut errors = ERRORS.write().unwrap();
    if errors.contains_key(whom) {
        errors.remove(whom);
        GENERATION.bump();
    }
}

pub fn from(whom: &str, wat: String) {
    let mut errors = ERRORS.write().unwrap();
    if !errors.contains_key(whom) {
        errors.insert(whom.to_owned(), Vec::new());
    }
    let vec = errors.get_mut(whom).unwrap();
    vec.push(wat);
    GENERATION.bump();
}

pub fn get() -> RwLockReadGuard<'static, BTreeMap<String,Vec<String>>> {
    ERRORS.read().unwrap()
}

pub fn generation() -> GenerationValue {
    GENERATION.snapshot()
}

pub fn if_newer_than(than: &GenerationValue) -> Option<(GenerationValue, RwLockReadGuard<'static, BTreeMap<String,Vec<String>>>)> {
    if GENERATION.has_not_changed_since(&than) { None }
    else {
        let locked = ERRORS.read().unwrap();
        if GENERATION.has_not_changed_since(&than) { None }
        else { Some((GENERATION.snapshot(), locked)) }
    }
}

pub fn clear_if_not_newer_than(than: &GenerationValue) {
    if !GENERATION.has_not_changed_since(&than) { return }
    else {
        let mut locked = ERRORS.write().unwrap();
        if !GENERATION.has_not_changed_since(&than) { return }
        else {
            locked.clear();
            GENERATION.bump();
        }
    }
}
