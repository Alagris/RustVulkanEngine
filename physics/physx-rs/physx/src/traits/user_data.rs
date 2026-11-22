use std::{ffi::c_void, ptr};

/// UserData allows easy access and initialization of userData *mut c_void fields on Px objects.
/// Not all Px objects with user data expose them as a field, so not all objects with user data can use this.
///
/// # Safety
///
/// All constructors of implementing types must call `init_user_data` during construction.
/// Values are boxed so their alignment, size, and drop behavior do not depend on the
/// representation of the native `userData` pointer field.
pub unsafe trait UserData: Sized {
    type UserData;
    /// Returns a reference to the userData field
    fn user_data_ptr(&self) -> &*mut c_void;
    /// Returns a mutable reference to the userData field.
    fn user_data_ptr_mut(&mut self) -> &mut *mut c_void;

    /// Boxes user data and stores its pointer in the native object.
    ///
    /// # Safety
    ///
    /// The field must not already own user data, and no references to its
    /// current value may be live.
    unsafe fn init_user_data(&mut self, user_data: Self::UserData) -> &mut Self {
        let field = self.user_data_ptr_mut();
        crate::invariant_assert!((*field).is_null(), "user data is already initialized");
        *field = Box::into_raw(Box::new(user_data)).cast();
        self
    }

    /// # Safety
    ///
    /// The user data field must have previously been initialized via `init_user_data`.
    unsafe fn get_user_data(this: &Self) -> &Self::UserData {
        let pointer = *this.user_data_ptr();
        crate::invariant_assert!(!pointer.is_null(), "user data is not initialized");
        unsafe { &*pointer.cast::<Self::UserData>() }
    }

    /// # Safety
    ///
    /// The user data field must have previously been initialized via `init_user_data`.
    unsafe fn get_user_data_mut(this: &mut Self) -> &mut Self::UserData {
        let pointer = *this.user_data_ptr_mut();
        crate::invariant_assert!(!pointer.is_null(), "user data is not initialized");
        unsafe { &mut *pointer.cast::<Self::UserData>() }
    }

    /// Drops the boxed user data and clears the native pointer field.
    ///
    /// # Safety
    ///
    /// The user data field must have been initialized exactly once with
    /// `init_user_data` and must not have been dropped already.
    unsafe fn drop_user_data(&mut self) {
        unsafe {
            let field = self.user_data_ptr_mut();
            let data = (*field).cast::<Self::UserData>();
            crate::invariant_assert!(!data.is_null(), "user data is already dropped");
            *field = ptr::null_mut();
            if !data.is_null() {
                drop(Box::from_raw(data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::c_void, fmt::Debug, marker::PhantomData, ptr::null_mut};

    use super::UserData;

    struct TestUserData<U> {
        user_data: *mut c_void,
        phantom: PhantomData<U>,
    }

    impl<U> Default for TestUserData<U> {
        fn default() -> Self {
            Self {
                user_data: null_mut(),
                phantom: PhantomData,
            }
        }
    }

    unsafe impl<U> UserData for TestUserData<U> {
        type UserData = U;

        fn user_data_ptr(&self) -> &*mut c_void {
            &self.user_data
        }

        fn user_data_ptr_mut(&mut self) -> &mut *mut c_void {
            &mut self.user_data
        }
    }

    fn do_test<U: PartialEq + Clone + Debug>(user_data: U) {
        unsafe {
            let mut object: TestUserData<U> = TestUserData::default();
            object.init_user_data(user_data.clone());

            assert_eq!(UserData::get_user_data(&object), &user_data);
            assert_eq!(UserData::get_user_data_mut(&mut object), &user_data);
            UserData::drop_user_data(&mut object);
        }
    }

    #[test]
    fn test_user_data() {
        do_test(()); // unit type
        do_test(100u8); // smaller than pointer
        do_test(100usize); // same size as pointer
        do_test([100usize; 4]); // larger than pointer
    }

    #[test]
    fn boxed_user_data_is_dropped() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        struct DropCounter {
            drops: Arc<AtomicUsize>,
            _padding: [u8; 16],
        }

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let mut object = TestUserData::<DropCounter>::default();
        unsafe {
            object.init_user_data(DropCounter {
                drops: drops.clone(),
                _padding: [0; 16],
            });
            object.drop_user_data();
        }

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pointer_sized_user_data_is_dropped() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        struct DropCounter(Arc<AtomicUsize>);

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let mut object = TestUserData::<DropCounter>::default();
        unsafe {
            object.init_user_data(DropCounter(drops.clone()));
            object.drop_user_data();
        }

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
