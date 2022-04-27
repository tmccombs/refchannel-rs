use std::fmt::Debug;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};

type InnerGuard<'a, T> = MutexGuard<'a, Option<NonNull<T>>>;

struct Inner<T: ?Sized> {
    lock: Mutex<Option<NonNull<T>>>,
    send_var: Condvar,
    recv_var: Condvar,
}

#[derive(Clone)]
pub struct RefChannel<T: ?Sized> {
    inner: Arc<Inner<T>>,
}

pub struct Guard<'a, T: ?Sized> {
    inner: InnerGuard<'a, T>,
    send_var: &'a Condvar,
}

impl<T: ?Sized + Sync> RefChannel<T> {
    /// # Safety
    /// This is only safe if the reference is kept alive until the receiver finishes using it, which
    /// generally means the thread needs to stay alive until this function returns.
    pub unsafe fn send(&self, t: &T) {
        // TODO: if the lock is poisoned, we should be able to
        // un-poison it
        let inner = &self.inner;
        let mut stored = false;
        let mut guard = inner.lock();
        loop {
            match &*guard {
                Some(_) => {
                    // the channel is full, wait for it to be empty
                    guard = inner.send_wait(guard);
                }
                None if !stored => {
                    *guard = Some(t.into());
                    inner.recv_var.notify_one(); // Notify that a value is available
                    stored = true;
                    guard = inner.send_wait(guard); // Now we need to wait for the receiver to finish
                }
                None => return,
            }
        }
    }

    pub fn recv(&self) -> Guard<'_, T> {
        let mut g = self.inner.lock();
        while g.is_none() {
            g = self.inner.recv_wait(g);
        }
        Guard {
            inner: g,
            send_var: &self.inner.send_var,
        }
    }
}

impl<T> Debug for RefChannel<T>
where
    T: ?Sized + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_tuple("RefChannel");
        match self.inner.lock.try_lock() {
            Ok(_guard) => unimplemented!(), // TODO: print the value if it is present
            Err(TryLockError::Poisoned(_)) => d.field(&"<poisoned>"),
            Err(TryLockError::WouldBlock) => d.field(&"<locked>"),
        };
        d.finish()
    }
}

impl<T: ?Sized + Sync> Deref for Guard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // Safety:
        // unwrap_unchecked is safe, because we already check that it is some when we create
        // the guard in `recv`.
        //
        // as_ref is safe, because we hold the lock until the guard is dropped, and the
        // caller of `send` garantees that the pointer will stay valid until this guard is dropped
        // and the lock is set to empty.
        //
        unsafe { self.inner.unwrap_unchecked().as_ref() }
    }
}

impl<T: ?Sized> Drop for Guard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        *self.inner = None;
        self.send_var.notify_all();
    }
}

impl<T: ?Sized> Inner<T> {
    // TODO: ideallly, we should be able to recover from a poisoned lock, but I'm not confident
    // that simply ignoring poisoning all the time is safe, because if we panic on the sender side,
    // we could end up with a dangling pointer. But there isn't a way to mark a mutex
    // as unpoisoned, so we can't just mark it as safe after writing to the mutex if it panics on
    // the receiver side.
    fn lock(&self) -> InnerGuard<'_, T> {
        //self.lock.lock().unwrap_or_else(|e| e.into_inner())
        self.lock.lock().unwrap()
    }

    fn send_wait<'a>(&self, g: InnerGuard<'a, T>) -> InnerGuard<'a, T> {
        //self.send_var.wait(g).unwrap_or_else(|e| e.into_inner())
        self.send_var.wait(g).unwrap()
    }

    fn recv_wait<'a>(&self, g: InnerGuard<'a, T>) -> InnerGuard<'a, T> {
        self.recv_var.wait(g).unwrap()
    }
}
