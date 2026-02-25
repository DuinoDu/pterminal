use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Bounded lock-free single-producer/single-consumer ring buffer.
///
/// The producer and consumer halves are intentionally non-cloneable to preserve
/// the SPSC contract.
pub(crate) fn channel<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let inner = Arc::new(Inner::new(capacity));
    (
        Producer {
            inner: Arc::clone(&inner),
        },
        Consumer { inner },
    )
}

pub(crate) struct Producer<T> {
    inner: Arc<Inner<T>>,
}

pub(crate) struct Consumer<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Producer<T> {
    pub(crate) fn try_push(&self, value: T) -> Result<(), T> {
        if self.inner.consumer_closed.load(Ordering::Acquire) {
            return Err(value);
        }

        let head = self.inner.head.load(Ordering::Relaxed);
        let tail = self.inner.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.inner.capacity {
            return Err(value);
        }

        let idx = head & self.inner.mask;
        // SAFETY: Only the single producer writes to the `head` slot before
        // publishing via `head.store(Release)`.
        unsafe {
            (*self.inner.buf[idx].get()).write(value);
        }
        self.inner
            .head
            .store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    pub(crate) fn push_blocking(&self, mut value: T) -> Result<(), T> {
        let mut spins = 0u32;
        loop {
            match self.try_push(value) {
                Ok(()) => return Ok(()),
                Err(v) => {
                    value = v;
                    if self.inner.consumer_closed.load(Ordering::Acquire) {
                        return Err(value);
                    }
                    if spins < 64 {
                        std::hint::spin_loop();
                    } else {
                        std::thread::yield_now();
                    }
                    spins = spins.wrapping_add(1);
                }
            }
        }
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.inner.producer_closed.store(true, Ordering::Release);
    }
}

impl<T> Consumer<T> {
    pub(crate) fn try_pop(&self) -> Option<T> {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        let head = self.inner.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }

        let idx = tail & self.inner.mask;
        // SAFETY: Only the single consumer reads from the `tail` slot after the
        // producer published it via `head.store(Release)`.
        let value = unsafe { (*self.inner.buf[idx].get()).assume_init_read() };
        self.inner
            .tail
            .store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub(crate) fn is_producer_closed(&self) -> bool {
        self.inner.producer_closed.load(Ordering::Acquire)
    }
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.inner.consumer_closed.store(true, Ordering::Release);
    }
}

struct Inner<T> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    producer_closed: AtomicBool,
    consumer_closed: AtomicBool,
}

impl<T> Inner<T> {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1).next_power_of_two();
        let mut buf = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buf.push(UnsafeCell::new(MaybeUninit::uninit()));
        }
        Self {
            buf: buf.into_boxed_slice(),
            mask: capacity - 1,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer_closed: AtomicBool::new(false),
            consumer_closed: AtomicBool::new(false),
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let tail = *self.tail.get_mut();
        let head = *self.head.get_mut();
        for idx in tail..head {
            let slot = idx & self.mask;
            // SAFETY: `tail..head` are initialized entries not yet consumed.
            unsafe {
                (*self.buf[slot].get()).assume_init_drop();
            }
        }
    }
}

// SAFETY: Access to ring slots is coordinated by the SPSC algorithm; `T: Send`
// is required because values cross threads.
unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}
