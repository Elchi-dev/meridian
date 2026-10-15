//! The frame allocator and the kernel's heap, in memory the kernel reaches
//! through the direct map, behind locks that are ready for more cores.

#[cfg(not(test))]
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
    KERNEL_ROOT.store(x86::raw::read_cr3() & !0xfff, Ordering::Release);
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

/// The kernel's level 4 table, which [`prepare_kernel_half`] finds in CR3
/// at boot; 0 before.
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// The kernel's level 4 table, the one CR3 held when the memory was set
/// up.
pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Acquire)
}

/// Gives every slot of the kernel's half of its level 4 table a table, so
/// an address space that copies that half sees whatever the kernel maps
/// there later. It stops early only when frames run out.
pub fn prepare_kernel_half() {
    let root = kernel_root();
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return;
    };
    let mut tables = KernelTables(frames);
    for slot in 256..512 {
        if paging::Frames::table(&mut tables, root)[slot] & Flags::PRESENT.0 != 0 {
            continue;
        }
        let Some(table) = paging::Frames::allocate(&mut tables) else {
            break;
        };
        paging::Frames::table(&mut tables, root)[slot] =
            table | (Flags::PRESENT | Flags::WRITABLE).0;
    }
}

/// Whether every slot of the kernel's half of its level 4 table has a
/// table. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn kernel_half_complete() -> bool {
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return false;
    };
    let root = *paging::Frames::table(&mut KernelTables(frames), kernel_root());
    root[256..]
        .iter()
        .all(|&entry| entry & Flags::PRESENT.0 != 0)
}

/// A new address space whose kernel half is the kernel's, and whose lower
/// half is empty: the physical address of its level 4 table.
pub fn new_space() -> Option<u64> {
    let mut frames = FRAMES.lock();
    let mut tables = KernelTables(frames.as_mut()?);
    let root = paging::Frames::allocate(&mut tables)?;
    let kernel = *paging::Frames::table(&mut tables, kernel_root());
    paging::Frames::table(&mut tables, root)[256..].copy_from_slice(&kernel[256..]);
    Some(root)
}

/// Gives back every frame of the lower half of the address space at
/// `root`, its tables, and `root` itself. Nothing may run on it any more.
pub fn release_space(root: u64) {
    let mut freed = alloc::vec::Vec::new();
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return;
    };
    AddressSpace::existing(root).release(&mut KernelTables(frames), 0..256, |f| freed.push(f));
    freed.push(root);
    for frame in freed {
        let _ = frames.free(frame);
    }
}

/// Runs `f` with the address of a page of ring 3 that holds a return, in
/// an address space of its own that is gone afterwards; none when no
/// memory is left for one. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn with_user_page<R>(f: impl FnOnce(u64) -> R) -> Option<R> {
    let root = new_space()?;
    let at = kernel::program::CODE_AT;
    if map_fresh_in(root, at, Flags::USER, &[0xc3]).is_err() {
        release_space(root);
        return None;
    }
    // A switch to another thread would load that thread's space.
    let result = x86::raw::without_interrupts(|| {
        let before = x86::raw::read_cr3();
        // SAFETY: the new space shares the kernel's half, where this
        // code, its stack and everything it reaches are.
        unsafe { x86::raw::load_cr3(root) };
        let result = f(at);
        // SAFETY: the space that was loaded before, which maps the same.
        unsafe { x86::raw::load_cr3(before) };
        result
    });
    release_space(root);
    Some(result)
}

/// Maps a fresh frame at `virt` in the address space at `root` with
/// `flags`, holding `contents` and zeros after them.
///
/// # Errors
///
/// When no frame is left or mapping fails; the frame goes back then.
pub fn map_fresh_in(
    root: u64,
    virt: u64,
    flags: Flags,
    contents: &[u8],
) -> Result<(), paging::Error> {
    let mut frames = FRAMES.lock();
    let frames = frames.as_mut().ok_or(paging::Error::OutOfFrames)?;
    let frame = frames.alloc().ok_or(paging::Error::OutOfFrames)?;
    let len = contents.len().min(4096);
    // SAFETY: a frame just taken from the allocator, so nothing else uses
    // it, in the direct map, and a whole frame long.
    let page = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + frame) as *mut u8, 4096) };
    page[..len].copy_from_slice(&contents[..len]);
    page[len..].fill(0);
    let mapped = AddressSpace::existing(root).map(
        &mut KernelTables(frames),
        virt,
        frame,
        PageSize::Size4K,
        flags,
    );
    if mapped.is_err() {
        let _ = frames.free(frame);
    }
    mapped
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
    let mut space = AddressSpace::existing(kernel_root());
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

/// Takes the 4 KiB page at `virt` out of the address space at `root`,
/// which runs on this processor, and gives its frame back.
///
/// # Errors
///
/// Those of [`AddressSpace::unmap`], such as nothing mapped there.
pub fn unmap_fresh_in(root: u64, virt: u64) -> Result<(), paging::Error> {
    let mut frames = FRAMES.lock();
    let frames = frames.as_mut().ok_or(paging::Error::OutOfFrames)?;
    let frame = AddressSpace::existing(root).unmap(&mut KernelTables(frames), virt)?;
    x86::raw::invalidate_page(virt);
    let _ = frames.free(frame);
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
    let mut space = AddressSpace::existing(kernel_root());
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

/// Whether every one of the `len` bytes at `at` lies in the user half and
/// is mapped for ring 3, so the kernel may read them for a system call.
pub fn user_readable(at: u64, len: u64) -> bool {
    user_mapped(at, len, Flags::USER)
}

/// Whether every one of the `len` bytes at `at` lies in the user half and
/// is mapped for ring 3 to write, so the kernel may write them for a
/// system call.
pub fn user_writable(at: u64, len: u64) -> bool {
    user_mapped(at, len, Flags::USER | Flags::WRITABLE)
}

/// Whether every one of the `len` bytes at `at` lies in the user half and
/// is mapped with `flags` in the address space running now.
fn user_mapped(at: u64, len: u64, flags: Flags) -> bool {
    kernel::syscall::in_user_half(at, len)
        && (len == 0
            || FRAMES.lock().as_mut().is_some_and(|frames| {
                let space = AddressSpace::existing(x86::raw::read_cr3() & !0xfff);
                let (first, last) = (at & !0xfff, (at + len - 1) & !0xfff);
                (first..=last).step_by(4096).all(|page| {
                    space
                        .translate(&mut KernelTables(frames), page)
                        .is_some_and(|(_, mapped, _)| mapped.contains(flags))
                })
            }))
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
