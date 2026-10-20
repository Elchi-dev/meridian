//! Shootdowns. When pages of a program go while another core runs one of
//! its threads, that core may still remember where they led and reach
//! their frames through it. It is asked to forget them with the SHOOTDOWN
//! interrupt, and the frames go back only once every core asked said it
//! did. One shootdown runs at a time; a core waiting to start its own
//! answers the one that runs meanwhile, so two cannot wait on each other.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::interrupts::{SHOOTDOWN, this_core};

/// Whether a shootdown runs.
static BUSY: AtomicBool = AtomicBool::new(false);

/// The address space, the first page and how many pages it is about.
static ROOT: AtomicU64 = AtomicU64::new(0);
static START: AtomicU64 = AtomicU64::new(0);
static PAGES: AtomicU64 = AtomicU64::new(0);

/// The cores that have yet to forget them, a bit each.
static PENDING: AtomicU64 = AtomicU64::new(0);

/// Forgets what this processor remembers of the pages the running
/// shootdown names, when it asks this core to, and says it did: on the
/// SHOOTDOWN interrupt, and while a core waits to start a shootdown.
pub fn serve() {
    let bit = 1u64 << this_core();
    #[cfg(feature = "qemu-test")]
    seen::looked(this_core());
    if PENDING.load(Ordering::Acquire) & bit == 0 {
        return;
    }
    if x86::raw::read_cr3() & !0xfff == ROOT.load(Ordering::Acquire) {
        let start = START.load(Ordering::Acquire);
        for page in 0..PAGES.load(Ordering::Acquire) {
            let at = start + page * 4096;
            x86::raw::invalidate_page(at);
            #[cfg(feature = "qemu-test")]
            seen::forgot(this_core(), at);
        }
    }
    PENDING.fetch_and(!bit, Ordering::AcqRel);
}

/// Makes every other core that runs a thread in the address space at
/// `root` forget the `pages` pages from `start` on, and returns once each
/// did. This core forgets them itself.
pub fn shoot_down(root: u64, start: u64, pages: u64) {
    let me = this_core();
    let cores = super::thread::cores_in(root, me);
    if cores == 0 {
        return;
    }
    while BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        serve();
        core::hint::spin_loop();
    }
    ROOT.store(root, Ordering::Release);
    START.store(start, Ordering::Release);
    PAGES.store(pages, Ordering::Release);
    PENDING.store(cores, Ordering::Release);
    for core in (0..64).filter(|core| cores & 1 << core != 0) {
        super::smp::interrupt(core, SHOOTDOWN);
    }
    while PENDING.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
    }
    BUSY.store(false, Ordering::Release);
}

/// What each core did for shootdowns. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub mod seen {
    use core::sync::atomic::{AtomicU64, Ordering};

    use kernel::CORES;

    /// How often each core looked for a shootdown to serve, how many pages
    /// it forgot, and the sum of their addresses, which tells them apart.
    static LOOKED: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
    static FORGOTTEN: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
    static SUM: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];

    pub(super) fn looked(core: usize) {
        LOOKED[core].fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn forgot(core: usize, page: u64) {
        FORGOTTEN[core].fetch_add(1, Ordering::AcqRel);
        SUM[core].fetch_add(page, Ordering::AcqRel);
    }

    /// How often core `core` looked, how many pages it forgot, and the sum
    /// of their addresses, wrapping.
    pub fn of(core: usize) -> (u64, u64, u64) {
        (
            LOOKED[core].load(Ordering::Acquire),
            FORGOTTEN[core].load(Ordering::Acquire),
            SUM[core].load(Ordering::Acquire),
        )
    }

    /// The cores the running shootdown still waits for, a bit each.
    pub fn pending() -> u64 {
        super::PENDING.load(Ordering::Acquire)
    }

    /// Whether a shootdown holds the right to run.
    pub fn busy() -> bool {
        super::BUSY.load(Ordering::Acquire)
    }
}
