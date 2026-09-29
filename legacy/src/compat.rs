//! Stable equivalents of the C2Rust 0.22 atomic intrinsics. These preserve the
//! relaxed access and sequentially consistent fence used by upstream SQLite.
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU16, AtomicU32, AtomicU8, Ordering};

pub(crate) trait AtomicValue: Sized {
    unsafe fn load(ptr: *mut Self) -> Self;
    unsafe fn store(ptr: *mut Self, value: Self);
}

macro_rules! atomic_value {
    ($value:ty, $atomic:ty) => {
        impl AtomicValue for $value {
            unsafe fn load(ptr: *mut Self) -> Self {
                <$atomic>::from_ptr(ptr).load(Ordering::Relaxed)
            }
            unsafe fn store(ptr: *mut Self, value: Self) {
                <$atomic>::from_ptr(ptr).store(value, Ordering::Relaxed)
            }
        }
    };
}
atomic_value!(i32, AtomicI32);
atomic_value!(u32, AtomicU32);
atomic_value!(u16, AtomicU16);
atomic_value!(u8, AtomicU8);

impl<T> AtomicValue for *mut T {
    unsafe fn load(ptr: *mut Self) -> Self {
        AtomicPtr::from_ptr(ptr).load(Ordering::Relaxed)
    }
    unsafe fn store(ptr: *mut Self, value: Self) {
        AtomicPtr::from_ptr(ptr).store(value, Ordering::Relaxed)
    }
}

type LogCallback = Option<unsafe extern "C" fn(*mut libc::c_void, i32, *const libc::c_char)>;
impl AtomicValue for LogCallback {
    unsafe fn load(ptr: *mut Self) -> Self {
        let raw = AtomicPtr::<libc::c_void>::from_ptr(ptr.cast()).load(Ordering::Relaxed);
        std::mem::transmute(raw)
    }
    unsafe fn store(ptr: *mut Self, value: Self) {
        AtomicPtr::<libc::c_void>::from_ptr(ptr.cast())
            .store(std::mem::transmute(value), Ordering::Relaxed)
    }
}

pub(crate) unsafe fn atomic_load_relaxed<T: AtomicValue>(ptr: *mut T) -> T {
    T::load(ptr)
}
pub(crate) unsafe fn atomic_store_relaxed<T: AtomicValue>(ptr: *mut T, value: T) {
    T::store(ptr, value);
}
pub(crate) fn atomic_fence_seqcst() {
    std::sync::atomic::fence(Ordering::SeqCst);
}
