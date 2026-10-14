//! The frame allocator and the kernel's heap, in memory the kernel reaches
//! through the direct map, behind locks that are ready for more cores.

#[cfg(not(test))]
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use bootinfo::{HHDM_BASE, MemoryRegion};
use frames::{FRAME, Frames};
use heap::{Heap, MAX_ALIGN};

/// A value one holder at a time may use, waited for by spinning.
pub struct Spin<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock lets one holder at a time reach the value, so sharing
// the lock between cores shares nothing else.
unsafe impl<T: Send> Sync for Spin<T> {}

impl<T> Spin<T> {
    pub const fn new(value: T) -> Spin<T> {
        Spin {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> Held<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Held { spin: self }
    }

    /// Whether someone holds the lock. Only the test image asks.
    #[cfg(feature = "qemu-test")]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire)
    }
}

/// The value of a [`Spin`] while it is held; dropping it lets go.
pub struct Held<'a, T> {
    spin: &'a Spin<T>,
}

impl<T> Deref for Held<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: holding the lock makes this the only way to the value.
        unsafe { &*self.spin.value.get() }
    }
}

impl<T> DerefMut for Held<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: holding the lock makes this the only way to the value.
        unsafe { &mut *self.spin.value.get() }
    }
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        self.spin.locked.store(false, Ordering::Release);
    }
}

/// The free frames of physical memory, once [`take`] has run.
pub static FRAMES: Spin<Option<Frames<'static>>> = Spin::new(None);

/// The heap and the address its region starts at in the direct map.
static HEAP: Spin<Option<(Heap<'static>, usize)>> = Spin::new(None);

static TAKEN: AtomicBool = AtomicBool::new(false);

/// Builds the frame allocator from the free memory of `regions` and gives
/// the kernel its heap from it. Returns how many frames are left free,
/// or `None` when there is no room for the allocator's bits or the heap,
/// or this ran before.
pub fn take(regions: &[MemoryRegion]) -> Option<usize> {
    if TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    let words = Frames::words_for(kernel::ram_top(regions));
    let bytes = words as u64 * 8;
    let home = kernel::bits_home(regions, bytes)?;
    // SAFETY: the bits lie in free memory above the first MiB, which the
    // loader handed over unused and the direct map covers; reserve takes
    // them out of the free frames at once, and TAKEN makes this the only
    // slice over them.
    let bits = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + home) as *mut u64, words) };
    let mut frames = Frames::new(bits, kernel::free_ranges(regions));
    frames.reserve(home, bytes);
    let heap_at = frames.alloc_run(kernel::HEAP_BYTES as u64 / FRAME, MAX_ALIGN as u64 / FRAME)?;
    let heap_words = Heap::words_for(kernel::HEAP_BYTES);
    let heap_bits_at = frames.alloc_run((heap_words as u64 * 8).div_ceil(FRAME), 1)?;
    // SAFETY: frames just taken from the allocator for this alone, in
    // the direct map.
    let heap_bits = unsafe {
        core::slice::from_raw_parts_mut((HHDM_BASE + heap_bits_at) as *mut u64, heap_words)
    };
    let free = frames.free_frames();
    let heap_base = usize::try_from(HHDM_BASE + heap_at).ok()?;
    *HEAP.lock() = Some((Heap::new(heap_bits, kernel::HEAP_BYTES), heap_base));
    *FRAMES.lock() = Some(frames);
    Some(free)
}

/// How many bytes of the heap are free. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn heap_free() -> usize {
    HEAP.lock().as_ref().map_or(0, |(h, _)| h.free_bytes())
}

/// The allocator behind Box, Vec and the rest of alloc in the kernel. The
/// host's tests keep the host's own.
#[cfg(not(test))]
struct KernelHeap;

// SAFETY: alloc hands out blocks the heap marks as taken, inside its
// region, which nothing else uses, at least as aligned as asked since the
// region starts on MAX_ALIGN; dealloc gives back what alloc handed out,
// with the size Rust promises it was asked for.
#[cfg(not(test))]
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = HEAP.lock();
        let Some((heap, base)) = heap.as_mut() else {
            return core::ptr::null_mut();
        };
        heap.alloc(layout.size(), layout.align())
            .map_or(core::ptr::null_mut(), |offset| (*base + offset) as *mut u8)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some((heap, base)) = HEAP.lock().as_mut() {
            let _ = heap.free(ptr as usize - *base, layout.size());
        }
    }
}

#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: KernelHeap = KernelHeap;

/// Fills the frame at `address` with `value`. Only the test image writes
/// memory this way, and only frames the allocator handed it.
#[cfg(feature = "qemu-test")]
pub fn fill_frame(address: u64, value: u64) {
    // SAFETY: the frame was handed out to the caller, lies in the direct
    // map and is a whole frame long.
    let frame = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + address) as *mut u64, 512) };
    frame.fill(value);
}

/// Whether every word of the frame at `address` holds `value`.
#[cfg(feature = "qemu-test")]
pub fn frame_holds(address: u64, value: u64) -> bool {
    // SAFETY: as for fill_frame; reading changes nothing.
    let frame = unsafe { core::slice::from_raw_parts((HHDM_BASE + address) as *const u64, 512) };
    frame.iter().all(|&w| w == value)
}
