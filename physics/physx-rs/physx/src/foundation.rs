// Author: Tom Olsson <tom.olsson@embark-studios.com>
// Copyright © 2019, Embark Studios, all rights reserved.
// Created: 10 April 2019

#![allow(non_upper_case_globals)]

use crate::{owner::Owner, traits::Class};

#[rustfmt::skip]
use physx_sys::{
    create_alloc_callback,
    get_alloc_callback_user_data,
    get_default_allocator,
    get_default_error_callback,
    phys_PxCreateFoundation,
    PxAllocatorCallback,
    PxErrorCallback,
    PxErrorCode,
    PxFoundation_getAllocatorCallback_mut,
    PxFoundation_getErrorCallback_mut,
    PxFoundation_getErrorLevel,
    PxFoundation_getReportAllocationNames,
    PxFoundation_release_mut,
    PxFoundation_setErrorLevel_mut,
    PxFoundation_setReportAllocationNames_mut,
};
use std::{
    alloc::{alloc, dealloc, Layout},
    ffi::c_void,
    fmt,
    marker::PhantomData,
    mem::{align_of, size_of},
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

#[cfg(any(test, feature = "memory-tracking"))]
pub mod memory_tracking {
    use std::{
        collections::HashSet,
        ffi::c_void,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex, OnceLock,
        },
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MemoryStats {
        pub allocations: usize,
        pub deallocations: usize,
        pub bytes_allocated: usize,
        pub bytes_deallocated: usize,
        pub live_allocations: usize,
        pub live_bytes: usize,
        pub invalid_deallocations: usize,
        pub duplicate_allocations: usize,
    }

    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static BYTES_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
    static BYTES_DEALLOCATED: AtomicUsize = AtomicUsize::new(0);
    static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
    static INVALID_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static DUPLICATE_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static LIVE_POINTERS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();

    fn live_pointers() -> &'static Mutex<HashSet<usize>> {
        LIVE_POINTERS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    pub fn snapshot() -> MemoryStats {
        let live_allocations = live_pointers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        MemoryStats {
            allocations: ALLOCATIONS.load(Ordering::Relaxed),
            deallocations: DEALLOCATIONS.load(Ordering::Relaxed),
            bytes_allocated: BYTES_ALLOCATED.load(Ordering::Relaxed),
            bytes_deallocated: BYTES_DEALLOCATED.load(Ordering::Relaxed),
            live_allocations,
            live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
            invalid_deallocations: INVALID_DEALLOCATIONS.load(Ordering::Relaxed),
            duplicate_allocations: DUPLICATE_ALLOCATIONS.load(Ordering::Relaxed),
        }
    }

    pub fn is_live(pointer: *const c_void) -> bool {
        !pointer.is_null()
            && live_pointers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&(pointer as usize))
    }

    pub fn assert_live(pointer: *const c_void) {
        assert!(
            is_live(pointer),
            "pointer {pointer:p} is not live in the PhysX allocator"
        );
    }

    pub fn assert_no_errors() {
        let stats = snapshot();
        assert_eq!(
            stats.invalid_deallocations, 0,
            "PhysX allocator received an unknown or already-freed pointer"
        );
        assert_eq!(
            stats.duplicate_allocations, 0,
            "PhysX allocator returned a pointer that is already live"
        );
    }

    pub(super) fn record_allocation(pointer: *mut c_void, size: usize) -> bool {
        let inserted = live_pointers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(pointer as usize);
        if !inserted {
            DUPLICATE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES_ALLOCATED.fetch_add(size, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
        true
    }

    pub(super) fn begin_deallocation(pointer: *const c_void) -> bool {
        let removed = live_pointers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(pointer as usize));
        if !removed {
            INVALID_DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    pub(super) fn record_deallocation(size: usize) {
        DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES_DEALLOCATED.fetch_add(size, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
    }
}

bitflags::bitflags! {
    #[repr(transparent)]
    pub struct ErrorCodes: u32 {
        /// An informational message.
        const DebugInfo = PxErrorCode::DebugInfo as u32;
        /// A warning message for the user to help with debugging
        const DebugWarning = PxErrorCode::DebugWarning as u32;
        /// Method called with invalid parameter(s)
        const InvalidParameter = PxErrorCode::InvalidParameter as u32;
        /// Method was called at a time when an operation is not possible
        const InvalidOperation = PxErrorCode::InvalidOperation as u32;
        /// Method failed to allocate some memory
        const OutOfMemory = PxErrorCode::OutOfMemory as u32;
        /// The library failed for some reason.
        ///
        /// Possibly you have passed invalid values like NaNs, which are not checked for.
        const InternalError = PxErrorCode::InternalError as u32;
        /// An unrecoverable error, execution should be halted and log output flushed
        const Abort = PxErrorCode::Abort as u32;
        /// The SDK has determined that an operation may result in poor performance.
        const PerfWarning = PxErrorCode::PerfWarning as u32;
    }
}

/// A new type wrapper for PxFoundation.
///
/// Parametrized by its Allocator type.
#[repr(transparent)]
pub struct PxFoundation<Allocator: AllocatorCallback> {
    obj: physx_sys::PxFoundation,
    phantom_interface: PhantomData<Allocator>,
}

impl<Allocator: AllocatorCallback> Drop for PxFoundation<Allocator> {
    fn drop(&mut self) {
        unsafe {
            let allocator = self
                .get_allocator_callback()
                .map(|allocator| allocator as *mut Allocator);
            PxFoundation_release_mut(self.as_mut_ptr());
            if let Some(allocator) = allocator {
                drop(Box::from_raw(allocator));
            }
        }
    }
}

unsafe impl<P, Allocator: AllocatorCallback> Class<P> for PxFoundation<Allocator>
where
    physx_sys::PxFoundation: Class<P>,
{
    fn as_ptr(&self) -> *const P {
        self.obj.as_ptr()
    }

    fn as_mut_ptr(&mut self) -> *mut P {
        self.obj.as_mut_ptr()
    }
}

unsafe impl<Allocator: AllocatorCallback + Send> Send for PxFoundation<Allocator> {}
unsafe impl<Allocator: AllocatorCallback + Sync> Sync for PxFoundation<Allocator> {}

impl<Allocator: AllocatorCallback> Foundation for PxFoundation<Allocator> {
    type Allocator = Allocator;
}

pub trait Foundation: Class<physx_sys::PxFoundation> + Sized {
    type Allocator: AllocatorCallback;

    /// Tries to create a PxFoundation of the given version with the default allocator and error callbacks.
    /// Returns `None` if `phys_PxCreateFoundation` returns a null pointer.
    fn new(allocator: Self::Allocator) -> Option<Owner<Self>> {
        unsafe {
            Self::with_allocator_error_callback(
                allocator,
                get_default_error_callback() as *mut PxErrorCallback,
            )
        }
    }

    /// Tries to create a PxFoundation with the provided allocator and error callbacks.
    /// Returns `None` if `phys_PxCreateFoundation` returns a null pointer.
    ///
    /// # Safety
    ///
    /// `error_callback` must live as long as the returned `Foundation`
    unsafe fn with_allocator_error_callback(
        allocator: Self::Allocator,
        error_callback: *mut PxErrorCallback,
    ) -> Option<Owner<Self>> {
        unsafe {
            Owner::from_raw(
                phys_PxCreateFoundation(
                    crate::physics::PX_PHYSICS_VERSION,
                    allocator.into_px(),
                    error_callback,
                )
                .cast::<Self>(),
            )
        }
    }

    /// Get the error callback.
    fn get_error_callback(&mut self) -> Option<&mut PxErrorCallback> {
        unsafe { PxFoundation_getErrorCallback_mut(self.as_mut_ptr()).as_mut() }
    }

    /// Set the error level.
    fn set_error_level(&mut self, mask: ErrorCodes) {
        unsafe { PxFoundation_setErrorLevel_mut(self.as_mut_ptr(), mask.bits()) }
    }

    /// Get the error level.
    fn get_error_level(&self) -> ErrorCodes {
        unsafe {
            ErrorCodes::from_bits(PxFoundation_getErrorLevel(self.as_ptr()))
                .expect("got invalid bits for error flags")
        }
    }

    /// Get the allocator callback.
    fn get_allocator_callback(&mut self) -> Option<&mut Self::Allocator> {
        unsafe {
            (get_alloc_callback_user_data(PxFoundation_getAllocatorCallback_mut(self.as_mut_ptr()))
                as *mut Self::Allocator)
                .as_mut()
        }
    }

    /// Get whether allocation names are reported.
    fn get_report_allocation_names(&self) -> bool {
        unsafe { PxFoundation_getReportAllocationNames(self.as_ptr()) }
    }

    /// Set whether allocation names are reported.
    fn set_report_allocation_names(&mut self, value: bool) {
        unsafe { PxFoundation_setReportAllocationNames_mut(self.as_mut_ptr(), value) }
    }
}

#[repr(align(16))]
struct ScratchBufferBlock {
    _bytes: [u8; 16 * 1024],
}

pub struct ScratchBuffer {
    ptr: NonNull<u8>,
    size: usize,
    layout: Option<Layout>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScratchBufferError {
    SizeOverflow,
    SizeTooLarge,
    AllocationFailed,
}

impl fmt::Display for ScratchBufferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeOverflow => formatter.write_str("scratch buffer size overflowed"),
            Self::SizeTooLarge => formatter.write_str("scratch buffer exceeds PhysX's u32 size"),
            Self::AllocationFailed => formatter.write_str("scratch buffer allocation failed"),
        }
    }
}

impl std::error::Error for ScratchBufferError {}

fn alloc_with_header(size: u64) -> (*mut c_void, usize) {
    let Some(allocation_size) = usize::try_from(size)
        .ok()
        .and_then(|size| size.checked_add(16))
    else {
        return (ptr::null_mut(), 0);
    };
    let Ok(layout) = Layout::from_size_align(allocation_size, 16) else {
        return (ptr::null_mut(), 0);
    };
    let allocation = unsafe { alloc(layout) };
    if allocation.is_null() {
        return (ptr::null_mut(), 0);
    }

    unsafe { allocation.cast::<u64>().write(allocation_size as u64) };
    (unsafe { allocation.add(16) as *mut c_void }, layout.size())
}

unsafe fn dealloc_with_header(ptr: *const c_void) -> (Layout, *mut u8) {
    let allocation = unsafe { (ptr as *mut u8).sub(16) };
    let allocation_size = unsafe { allocation.cast::<u64>().read() as usize };
    let layout = Layout::from_size_align(allocation_size, 16)
        .expect("allocator header must contain a valid layout");
    (layout, allocation)
}

impl ScratchBuffer {
    pub fn try_new(nb_blocks: usize) -> Result<Self, ScratchBufferError> {
        let size = size_of::<ScratchBufferBlock>()
            .checked_mul(nb_blocks)
            .ok_or(ScratchBufferError::SizeOverflow)?;
        if size > u32::MAX as usize {
            return Err(ScratchBufferError::SizeTooLarge);
        }
        if size == 0 {
            return Ok(Self {
                ptr: NonNull::dangling(),
                size,
                layout: None,
            });
        }

        let layout = Layout::from_size_align(size, align_of::<ScratchBufferBlock>())
            .map_err(|_| ScratchBufferError::SizeOverflow)?;
        let ptr =
            NonNull::new(unsafe { alloc(layout) }).ok_or(ScratchBufferError::AllocationFailed)?;
        Ok(Self {
            ptr,
            size,
            layout: Some(layout),
        })
    }

    #[inline]
    pub(crate) fn as_ptr_and_size(&mut self) -> (*mut c_void, u32) {
        if self.size == 0 {
            (ptr::null_mut(), 0)
        } else {
            (self.ptr.as_ptr() as *mut c_void, self.size as u32)
        }
    }

    /// # Safety
    ///
    /// The buffer must live at least until fetch_results returns.
    pub unsafe fn new(nb_blocks: usize) -> Self {
        Self::try_new(nb_blocks).unwrap_or_else(|error| panic!("{error}"))
    }
}

impl Drop for ScratchBuffer {
    fn drop(&mut self) {
        if let Some(layout) = self.layout {
            unsafe { dealloc(self.ptr.as_ptr(), layout) };
        }
    }
}

/// A trait for creating allocator callbacks for PhysX.
///
/// Reporting the name, file and line is not enabled by default.
/// Use [`Foundation::set_report_allocation_names`] to toggle this on or off.
#[allow(clippy::missing_safety_doc)]
pub unsafe trait AllocatorCallback: Sized {
    /// # Safety
    ///
    /// Allocations must be aligned 16. This should not panic, since it is
    /// called in an FFI context and unwinding across the FFI barrier is UB.
    unsafe extern "C" fn allocate(
        size: u64,
        name: *const c_void,
        file: *const c_void,
        line: u32,
        user_data: *const c_void,
    ) -> *mut c_void;

    /// # Safety
    ///
    /// Must not panic.
    unsafe extern "C" fn deallocate(ptr: *const c_void, user_data: *const c_void);

    /// # Safety
    ///
    /// Do not override this method.
    unsafe fn into_px(self) -> *mut PxAllocatorCallback {
        unsafe {
            create_alloc_callback(
                Self::allocate,
                Self::deallocate,
                Box::into_raw(Box::new(self)) as *mut c_void,
            )
        }
    }
}

/// A very simple allocator that calls through to Rust's GlobalAllocator.
pub struct GlobalAllocCallback;

unsafe impl AllocatorCallback for GlobalAllocCallback {
    unsafe extern "C" fn allocate(
        size: u64,
        _name: *const c_void,
        _file: *const c_void,
        _line: u32,
        _user_data: *const c_void,
    ) -> *mut c_void {
        let (allocation, _allocation_size) = alloc_with_header(size);
        #[cfg(any(test, feature = "memory-tracking"))]
        if !allocation.is_null()
            && !memory_tracking::record_allocation(allocation, _allocation_size)
        {
            return ptr::null_mut();
        }
        allocation
    }

    unsafe extern "C" fn deallocate(ptr: *const c_void, _user_data: *const c_void) {
        if ptr.is_null() {
            return;
        }
        #[cfg(any(test, feature = "memory-tracking"))]
        if !memory_tracking::begin_deallocation(ptr) {
            return;
        }
        let (layout, allocation) = unsafe { dealloc_with_header(ptr) };
        #[cfg(any(test, feature = "memory-tracking"))]
        memory_tracking::record_deallocation(layout.size());
        unsafe { dealloc(allocation, layout) };
    }
}

/// A simple tracking allocator that calls through to Rust's GlobalAllocator.
/// Note that it tracks the amount of space needed for storing the allocation sizes
/// as well as the actual data.
pub struct TrackingAllocator {
    pub allocated: AtomicUsize,
    pub deallocated: AtomicUsize,
}

impl Default for TrackingAllocator {
    fn default() -> Self {
        Self {
            allocated: AtomicUsize::new(0),
            deallocated: AtomicUsize::new(0),
        }
    }
}

unsafe impl AllocatorCallback for TrackingAllocator {
    unsafe extern "C" fn allocate(
        size: u64,
        _name: *const c_void,
        _file: *const c_void,
        _line: u32,
        user_data: *const c_void,
    ) -> *mut c_void {
        let user_data = unsafe { &*(user_data as *const Self) };
        let (allocation, size) = alloc_with_header(size);
        if !allocation.is_null() {
            #[cfg(any(test, feature = "memory-tracking"))]
            if !memory_tracking::record_allocation(allocation, size) {
                return ptr::null_mut();
            }
            user_data.allocated.fetch_add(size, SeqCst);
        }
        allocation
    }

    unsafe extern "C" fn deallocate(ptr: *const c_void, user_data: *const c_void) {
        if ptr.is_null() {
            return;
        }
        #[cfg(any(test, feature = "memory-tracking"))]
        if !memory_tracking::begin_deallocation(ptr) {
            return;
        }
        let (layout, allocation) = unsafe { dealloc_with_header(ptr) };
        #[cfg(any(test, feature = "memory-tracking"))]
        memory_tracking::record_deallocation(layout.size());
        let user_data = unsafe { &*(user_data as *const Self) };
        user_data.deallocated.fetch_add(layout.size(), SeqCst);
        unsafe { dealloc(allocation, layout) };
    }
}

/// The PhysX-provided default allocator.
pub struct DefaultAllocator;

unsafe impl AllocatorCallback for DefaultAllocator {
    unsafe fn into_px(self) -> *mut PxAllocatorCallback {
        unsafe { get_default_allocator() as *mut PxAllocatorCallback }
    }

    unsafe extern "C" fn allocate(
        _size: u64,
        _name: *const c_void,
        _file: *const c_void,
        _line: u32,
        _user_data: *const c_void,
    ) -> *mut c_void {
        unreachable!()
    }

    unsafe extern "C" fn deallocate(_ptr: *const c_void, _user_data: *const c_void) {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::{memory_tracking, ScratchBuffer, ScratchBufferError};
    use std::{
        alloc::{alloc, dealloc, Layout},
        panic::{catch_unwind, AssertUnwindSafe},
        ptr,
    };

    #[test]
    fn zero_sized_scratch_buffer_has_no_allocation() {
        let mut buffer = ScratchBuffer::try_new(0).unwrap();
        assert_eq!(buffer.as_ptr_and_size(), (ptr::null_mut(), 0));
    }

    #[test]
    fn scratch_buffer_rejects_sizes_that_physx_cannot_represent() {
        let blocks = (u32::MAX as usize / 16_384) + 1;
        assert!(matches!(
            ScratchBuffer::try_new(blocks).err(),
            Some(ScratchBufferError::SizeTooLarge | ScratchBufferError::SizeOverflow)
        ));
    }

    #[test]
    fn scratch_buffer_rejects_size_overflow() {
        assert_eq!(
            ScratchBuffer::try_new(usize::MAX).err(),
            Some(ScratchBufferError::SizeOverflow)
        );
    }

    #[test]
    fn pointer_tracker_detects_use_after_free_and_double_free() {
        let layout = Layout::from_size_align(32, 16).unwrap();
        let pointer = unsafe { alloc(layout) };
        assert!(!pointer.is_null());

        let before = memory_tracking::snapshot();
        assert!(memory_tracking::record_allocation(
            pointer.cast(),
            layout.size()
        ));
        assert!(!memory_tracking::record_allocation(
            pointer.cast(),
            layout.size()
        ));
        memory_tracking::assert_live(pointer.cast());

        assert!(memory_tracking::begin_deallocation(pointer.cast()));
        memory_tracking::record_deallocation(layout.size());
        unsafe { dealloc(pointer, layout) };

        assert!(catch_unwind(AssertUnwindSafe(|| {
            memory_tracking::assert_live(pointer.cast())
        }))
        .is_err());
        assert!(!memory_tracking::begin_deallocation(pointer.cast()));

        let after = memory_tracking::snapshot();
        assert_eq!(after.live_allocations, before.live_allocations);
        assert_eq!(
            after.invalid_deallocations,
            before.invalid_deallocations + 1
        );
        assert_eq!(
            after.duplicate_allocations,
            before.duplicate_allocations + 1
        );
    }
}
