use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::{Deref, DerefMut};

pub struct SpinLock<T: ?Sized> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

pub struct SpinLockGuard<'a, T: ?Sized + 'a> {
    owner: &'a SpinLock<T>,
}


impl<T> SpinLock<T> {
    pub fn new(data: T) -> SpinLock<T> {
        SpinLock {
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> SpinLock<T> {
    pub fn lock(&self) -> SpinLockGuard<T> {
        while self.lock.compare_and_swap(false, true, Ordering::Acquire) {
            std::thread::yield_now();
        }
        SpinLockGuard {
            owner: self
        }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<T>> {
        if self.lock.compare_and_swap(false, true, Ordering::Acquire) {
            None
        } else {
            Some(SpinLockGuard {
                owner: self
            })
        }
    }
}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> SpinLock<T> {
        SpinLock {
            lock: AtomicBool::new(false),
            data: UnsafeCell::default(),
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for SpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(guard) = self.try_lock() {
            f.debug_struct("SpinLock").field("data", &&*guard).finish()
        } else {
            struct LockedPlaceholder;
            impl fmt::Debug for LockedPlaceholder {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("<locked>")
                }
            }
            f.debug_struct("SpinLock").field("data", &LockedPlaceholder).finish()
        }
    }
}

unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}
unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}

impl<'a, T: ?Sized> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        self.owner.lock.store(false, Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.owner.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.owner.data.get() }
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for SpinLockGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized> !Sync for SpinLockGuard<'a, T> {}

