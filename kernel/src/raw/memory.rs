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
use paging::{AddressSpace, Flags, PageSize};

/// A value one holder at a time may use, waited for by spinning. While it
/// is held, interrupts are off, so the timer cannot hand the processor to
/// a thread that would then spin for the lock its holder cannot let go.
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
        let interrupts = x86::raw::interrupts_enabled();
        x86::raw::disable_interrupts();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Held {
            spin: self,
            interrupts,
        }
    }

    /// Whether someone holds the lock. Only the test image asks.
    #[cfg(feature = "qemu-test")]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire)
    }
}

/// The value of a [`Spin`] while it is held; dropping it lets go, and
/// turns interrupts back on if they were on before.
pub struct Held<'a, T> {
    spin: &'a Spin<T>,
    interrupts: bool,
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
        if self.interrupts {
            // SAFETY: interrupts were on before the lock was taken, so
            // whoever handles them was ready then and still is.
            unsafe { x86::raw::enable_interrupts() };
        }
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

/// The kernel's page tables, reached through the direct map, with new
/// tables from the frame allocator.
struct KernelTables<'f>(&'f mut Frames<'static>);

impl paging::Frames for KernelTables<'_> {
    fn allocate(&mut self) -> Option<u64> {
        let frame = self.0.alloc()?;
        // SAFETY: a frame just taken from the allocator, so nothing else
        // uses it, in the direct map.
        unsafe { core::ptr::write_bytes((HHDM_BASE + frame) as *mut u8, 0, 4096) };
        Some(frame)
    }

    fn table(&mut self, physical: u64) -> &mut [u64; 512] {
        // SAFETY: physical is the kernel's root table or one its entries
        // lead to, all in memory the direct map covers; the FRAMES lock,
        // which this holds, is the only way to change them.
        unsafe { &mut *((HHDM_BASE + physical) as *mut [u64; 512]) }
    }
}

/// Maps the frame at `phys` at `virt` in the kernel's address space as a
/// 4 KiB page with `flags`.
///
/// # Errors
///
/// Those of [`AddressSpace::map`], and running out of frames before the
/// allocator is there.
pub fn map_page(virt: u64, phys: u64, flags: Flags) -> Result<(), paging::Error> {
    let mut frames = FRAMES.lock();
    let frames = frames.as_mut().ok_or(paging::Error::OutOfFrames)?;
    let mut space = AddressSpace::existing(x86::raw::read_cr3() & !0xfff);
    space.map(
        &mut KernelTables(frames),
        virt,
        phys,
        PageSize::Size4K,
        flags,
    )
}

/// Maps `pages` fresh frames as a stack that ends at `top`. What lies
/// below stays as it is, so a stack with nothing mapped below faults when
/// it runs over.
///
/// # Errors
///
/// Those of [`map_page`], and running out of frames. Then nothing of the
/// stack stays mapped and its frames go back.
pub fn map_stack(top: u64, pages: u64) -> Result<(), paging::Error> {
    let flags = Flags::WRITABLE | Flags::NO_EXECUTE;
    for page in 1..=pages {
        let frame = FRAMES.lock().as_mut().and_then(Frames::alloc);
        let mapped = frame.map_or(Err(paging::Error::OutOfFrames), |f| {
            map_page(top - page * FRAME, f, flags)
        });
        if let Err(e) = mapped {
            let give_back = |f: u64| {
                if let Some(frames) = FRAMES.lock().as_mut() {
                    let _ = frames.free(f);
                }
            };
            if let Some(f) = frame {
                give_back(f);
            }
            for p in 1..page {
                if let Ok(f) = unmap_page(top - p * FRAME) {
                    give_back(f);
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Takes the 4 KiB page at `virt` out of the kernel's address space,
/// drops what this processor remembers of it, and returns its frame.
///
/// # Errors
///
/// Those of [`AddressSpace::unmap`], and missing frames as for
/// [`map_page`].
pub fn unmap_page(virt: u64) -> Result<u64, paging::Error> {
    let mut frames = FRAMES.lock();
    let frames = frames.as_mut().ok_or(paging::Error::OutOfFrames)?;
    let mut space = AddressSpace::existing(x86::raw::read_cr3() & !0xfff);
    let frame = space.unmap(&mut KernelTables(frames), virt)?;
    x86::raw::invalidate_page(virt);
    Ok(frame)
}

/// Where `virt` leads in the kernel's address space, and with which flags.
/// Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn translate(virt: u64) -> Option<(u64, Flags)> {
    let mut frames = FRAMES.lock();
    let frames = frames.as_mut()?;
    let space = AddressSpace::existing(x86::raw::read_cr3() & !0xfff);
    space
        .translate(&mut KernelTables(frames), virt)
        .map(|(physical, flags, _)| (physical, flags))
}

/// Maps a fresh frame at `virt` with `flags`, holding `contents` and zeros
/// after them.
///
/// # Errors
///
/// When no frame is left or [`map_page`] fails; the frame goes back then.
pub fn map_fresh(virt: u64, flags: Flags, contents: &[u8]) -> Result<(), paging::Error> {
    let frame = FRAMES
        .lock()
        .as_mut()
        .and_then(Frames::alloc)
        .ok_or(paging::Error::OutOfFrames)?;
    let len = contents.len().min(4096);
    // SAFETY: a frame just taken from the allocator, so nothing else uses
    // it, in the direct map, and a whole frame long.
    let page = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + frame) as *mut u8, 4096) };
    page[..len].copy_from_slice(&contents[..len]);
    page[len..].fill(0);
    map_page(virt, frame, flags).inspect_err(|_| {
        if let Some(frames) = FRAMES.lock().as_mut() {
            let _ = frames.free(frame);
        }
    })
}

/// Takes the page at `virt` out of the kernel's address space and gives
/// its frame back; nothing happens where nothing is mapped.
pub fn unmap_and_free(virt: u64) {
    if let Ok(frame) = unmap_page(virt)
        && let Some(frames) = FRAMES.lock().as_mut()
    {
        let _ = frames.free(frame);
    }
}

/// Whether every one of the `len` bytes at `at` lies in the user half and
/// is mapped for ring 3, so the kernel may read them for a system call.
pub fn user_readable(at: u64, len: u64) -> bool {
    if !kernel::syscall::in_user_half(at, len) {
        return false;
    }
    if len == 0 {
        return true;
    }
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return false;
    };
    let space = AddressSpace::existing(x86::raw::read_cr3() & !0xfff);
    let (first, last) = (at & !0xfff, (at + len - 1) & !0xfff);
    (first..=last).step_by(4096).all(|page| {
        space
            .translate(&mut KernelTables(frames), page)
            .is_some_and(|(_, flags, _)| flags.contains(Flags::USER))
    })
}

/// The 8 bytes at `virt`. Only the test image reads memory this way, and
/// only pages it mapped itself or frames in the direct map.
#[cfg(feature = "qemu-test")]
pub fn read_word(virt: u64) -> u64 {
    // SAFETY: the test reads only what it mapped or a frame it holds, and
    // 8 aligned bytes lie inside the page.
    unsafe { core::ptr::read_volatile(virt as *const u64) }
}

/// Writes the 8 bytes at `virt`, under the same rule as [`read_word`].
#[cfg(feature = "qemu-test")]
pub fn write_word(virt: u64, value: u64) {
    // SAFETY: as for read_word; the page is the test's own frame.
    unsafe { core::ptr::write_volatile(virt as *mut u64, value) };
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
