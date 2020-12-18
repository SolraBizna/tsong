//! Every time a module that "keeps track of things" has an entry added,
//! removed, or changed, its generation number gets incremented. This allows
//! other modules to passively pick up on the fact that they may need to
//! update.
//!
//! (This is not used to coordinate `PhysicalFile`s and `LogicalSong`s, since
//! they have a more explicit, direct relationship.)

use std::{
    fmt,
    fmt::{Display, Debug, Formatter},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Tracks generation numbers.
pub struct GenerationTracker {
    n: AtomicUsize,
}

/// A particular value of a generation tracker at a particular time. If it
/// matches a tracker before and after an operation, then one of the following
/// are guaranteed:
/// - Our view into the world was coherent during the whole operation
/// - The tracker is going to be bumped, later, and we can try again
pub struct GenerationValue {
    n: usize,
}

/// A special generation number that indicates that nothing has been touched
/// yet. (zero)
pub const NOT_GENERATED: GenerationValue = GenerationValue { n: 0 };

impl Display for GenerationValue {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.write_fmt(format_args!("{}", self.n))
    }
}

impl Debug for GenerationValue {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, fmt)
    }
}

impl GenerationTracker {
    /// Creates a new `NOT_GENERATED` tracker.
    pub const fn new() -> GenerationTracker {
        GenerationTracker { n: AtomicUsize::new(0) }
    }
    /// Indicate that updates have been completed, and a new, consistent state
    /// is now in place.
    pub fn bump(&self) {
        self.n.fetch_add(1, Ordering::Release);
    }
    /// Get the current GenerationValue. sort of.
    pub fn snapshot(&self) -> GenerationValue {
        GenerationValue { n: self.n.load(Ordering::Acquire) }
    }
    /// Return true if the given GenerationValue is current.
    pub fn has_not_changed_since(&self, other: &GenerationValue) -> bool {
        self.n.load(Ordering::Acquire) == other.n
    }
}

impl GenerationValue {
    /// Equivalent to assigning `NOT_GENERATED`; sets this generation number to
    /// a value that no (touched) module will consider current.
    pub fn destroy(&mut self) {
        *self = NOT_GENERATED
    }
}

impl Default for GenerationValue {
    fn default() -> GenerationValue {
        NOT_GENERATED
    }
}
