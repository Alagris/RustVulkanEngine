use std::ptr::{drop_in_place, NonNull};

#[cfg(any(test, feature = "memory-tracking"))]
use std::sync::{Mutex, OnceLock};

use crate::traits::Class;

#[cfg(any(test, feature = "memory-tracking"))]
static LIVE_OWNERS: OnceLock<Mutex<std::collections::HashSet<usize>>> = OnceLock::new();

#[cfg(any(test, feature = "memory-tracking"))]
fn owner_registry() -> &'static Mutex<std::collections::HashSet<usize>> {
    LIVE_OWNERS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(any(test, feature = "memory-tracking"))]
fn register_owner(pointer: *mut ()) {
    let inserted = owner_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(pointer as usize);
    crate::invariant_assert!(
        inserted,
        "multiple Rust owners wrap the same native pointer"
    );
}

#[cfg(not(any(test, feature = "memory-tracking")))]
fn register_owner(_pointer: *mut ()) {}

#[cfg(any(test, feature = "memory-tracking"))]
fn assert_owner_live(pointer: *const ()) {
    let is_live = owner_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&(pointer as usize));
    crate::invariant_assert!(is_live, "native pointer used after its owner was dropped");
}

#[cfg(not(any(test, feature = "memory-tracking")))]
fn assert_owner_live(_pointer: *const ()) {}

#[cfg(any(test, feature = "memory-tracking"))]
fn unregister_owner(pointer: *mut ()) {
    let removed = owner_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&(pointer as usize));
    crate::invariant_assert!(removed, "native pointer owner was already released");
}

#[cfg(not(any(test, feature = "memory-tracking")))]
fn unregister_owner(_pointer: *mut ()) {}

/// An owning wrapper around a pointer.  PhysX object creation returns pointers
/// which in normal Rust would be a Box, but Box cannot be used here because it
/// has specific allocation requirements that the C++ code does not fulfill.
#[repr(transparent)]
pub struct Owner<T> {
    ptr: NonNull<T>,
}

impl<T> Owner<T> {
    /// Create a new owner from a raw pointer.  Use the `from_raw` method on the type!
    ///
    /// # Safety
    ///
    /// Only one of these may be constructed per pointer.  In particular,
    /// this must not be called on a pointer attained via Class::as_mut_ptr.  The
    /// intended pattern is to wrap the raw FFI constructors in this call, so that
    /// the amount of time spent dealing with raw pointers is as little as possible.
    pub(crate) unsafe fn from_raw(ptr: *mut T) -> Option<Self> {
        if !ptr.is_null() {
            crate::invariant_assert!(
                (ptr as usize) % std::mem::align_of::<T>() == 0,
                "native pointer is misaligned for its Rust wrapper"
            );
        }
        let ptr = NonNull::new(ptr)?;
        register_owner(ptr.as_ptr().cast());
        Some(Self { ptr })
    }

    /// Consumes the Owner without calling Drop and returns the raw pointer it was wrapping.
    #[allow(clippy::mem_forget)]
    pub fn into_ptr<S>(mut self) -> *mut S
    where
        T: Class<S>,
    {
        let ptr = self.as_mut_ptr();
        crate::invariant_assert!(!ptr.is_null(), "owner pointer unexpectedly became null");
        unregister_owner(ptr.cast());
        std::mem::forget(self);
        ptr
    }
}

unsafe impl<T, S> Class<S> for Owner<T>
where
    T: Class<S>,
{
    fn as_ptr(&self) -> *const S {
        self.as_ref().as_ptr()
    }

    fn as_mut_ptr(&mut self) -> *mut S {
        self.as_mut().as_mut_ptr()
    }
}

unsafe impl<T: Send> Send for Owner<T> {}
unsafe impl<T: Sync> Sync for Owner<T> {}

impl<T> Drop for Owner<T> {
    fn drop(&mut self) {
        let pointer = self.ptr.as_ptr();
        unregister_owner(pointer.cast());
        unsafe { drop_in_place(pointer) }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        alloc::{dealloc, Layout},
        panic::{catch_unwind, AssertUnwindSafe},
    };

    use super::{assert_owner_live, Owner};
    use crate::traits::Class;

    #[repr(transparent)]
    struct TestObject(u64);

    unsafe impl Class<TestObject> for TestObject {
        fn as_ptr(&self) -> *const TestObject {
            self
        }

        fn as_mut_ptr(&mut self) -> *mut TestObject {
            self
        }
    }

    #[test]
    fn owner_registry_rejects_access_after_drop() {
        let pointer = Box::into_raw(Box::new(TestObject(42)));
        let owner = unsafe { Owner::from_raw(pointer) }.unwrap();
        assert_eq!(owner.0, 42);
        drop(owner);

        assert!(catch_unwind(AssertUnwindSafe(|| { assert_owner_live(pointer.cast()) })).is_err());

        unsafe { dealloc(pointer.cast(), Layout::new::<TestObject>()) };
    }

    #[test]
    fn owner_registry_rejects_duplicate_owners() {
        let pointer = Box::into_raw(Box::new(TestObject(42)));
        let owner = unsafe { Owner::from_raw(pointer) }.unwrap();

        assert!(catch_unwind(AssertUnwindSafe(|| unsafe { Owner::from_raw(pointer) })).is_err());

        drop(owner);
        unsafe { dealloc(pointer.cast(), Layout::new::<TestObject>()) };
    }
}

impl<T> AsRef<T> for Owner<T> {
    fn as_ref(&self) -> &T {
        assert_owner_live(self.ptr.as_ptr().cast());
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> AsMut<T> for Owner<T> {
    fn as_mut(&mut self) -> &mut T {
        assert_owner_live(self.ptr.as_ptr().cast());
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> std::ops::Deref for Owner<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> std::ops::DerefMut for Owner<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}
