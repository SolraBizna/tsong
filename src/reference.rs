//! Provides the `Reference<T>` type, a convenient, debug-friendly wrapper
//! around `Arc<RwLock<T>>`

use std::{
    fmt, fmt::{Debug, Formatter},
    hash::{Hash, Hasher},
    ops::Deref,
    sync::{Arc, RwLock},
    any::type_name,
};

/// A reference-counted (`Arc`) pointer to a lockable (`RwLock`) object.
/// Provides an implementation of `Debug`, as well as implementations of `Eq`
/// and `Hash` that work with the *pointer*—objects are equal if they are
/// literally the same instance.
pub struct Reference<T: Debug> {
    inner: Arc<RwLock<T>>,
}

impl<T: Debug> Reference<T> {
    pub fn new(inner: T) -> Reference<T> {
        Reference {
            inner: Arc::new(RwLock::new(inner))
        }
    }
}

impl<T: Debug> Debug for Reference<T> {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self.inner.try_read() {
            Ok(inner) => {
                fmt.write_str("&<")?;
                (*inner).fmt(fmt)?;
                fmt.write_str(">")
            },
            Err(_) => {
                write!(fmt, "&<locked {} ref>", type_name::<T>())
            },
        }
    }
}

impl<T: Debug> Clone for Reference<T> {
    fn clone(&self) -> Reference<T> {
        Reference { inner: self.inner.clone() }
    }
}

impl<T: Debug> PartialEq for Reference<T> {
    fn eq(&self, other: &Reference<T>) -> bool {
        let a_as_ptr: *const RwLock<T> = &***self;
        let b_as_ptr: *const RwLock<T> = &***other;
        a_as_ptr == b_as_ptr
    }
}

impl<T: Debug> Eq for Reference<T> {}

impl<T: Debug> Hash for Reference<T> {
    fn hash<H>(&self, hasher: &mut H) where H: Hasher {
        let as_ptr: *const RwLock<T> = &***self;
        hasher.write_usize(as_ptr as usize);
    }
}

impl<T: Debug> Deref for Reference<T> {
    type Target = Arc<RwLock<T>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
