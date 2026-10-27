//! What takes unsafe code: the one way into the kernel, `int 0x2e`, the
//! heap behind `alloc`, and the start of a program, which the linker has
//! to find by its name.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use abi::{Call, Error};

use crate::Pool;

/// Makes the system call `call`, and returns what the kernel left in rax
/// and rdx.
pub fn call(call: Call) -> (u64, u64) {
    let (number, [first, second, third, fourth]) = call.encode();
    let (rax, rdx);
    // SAFETY: the kernel keeps every register but rax and rdx, touches
    // only the memory a call names, after checking it is the program's,
    // and runs the call on a stack of its own.
    unsafe {
        core::arch::asm!(
            "int 0x2e",
            inlateout("rax") number => rax,
            in("rdi") first,
            in("rsi") second,
            inlateout("rdx") third => rdx,
            in("r10") fourth,
            options(nostack),
        );
    }
    (rax, rdx)
}

/// The arguments [`keep_arguments`] kept; none before.
static ARGUMENTS: AtomicPtr<Vec<String>> = AtomicPtr::new(core::ptr::null_mut());

/// Keeps `arguments` for the rest of the program, for [`arguments`].
pub fn keep_arguments(arguments: Vec<String>) {
    ARGUMENTS.store(Box::into_raw(Box::new(arguments)), Ordering::SeqCst);
}

/// The program's arguments, as its parent handed them over (decision
/// record 0018), without the program's own name.
#[must_use]
pub fn arguments() -> &'static [String] {
    let kept = ARGUMENTS.load(Ordering::SeqCst);
    if kept.is_null() {
        return &[];
    }
    // SAFETY: the pointer came from Box::into_raw in keep_arguments, and
    // nothing frees it, so it stays valid while the program runs.
    unsafe { &*kept }
}

/// What [`start_thread`] hands a new thread: the work it does.
type Work = Box<dyn FnOnce() -> u64 + Send>;

/// Starts another thread of this program that does `work`, with its stack
/// pointer at `stack`.
///
/// # Errors
///
/// When the kernel starts no thread.
pub fn start_thread(work: Work, stack: u64) -> Result<(), Error> {
    let start = Box::into_raw(Box::new(work));
    let call = Call::Thread {
        entry: thread_entry as *const () as usize as u64,
        stack,
        argument: start as u64,
    };
    abi::answer(self::call(call).0)
        .map(|_| ())
        .inspect_err(|_| {
            // SAFETY: the box made just above, which no thread took.
            drop(unsafe { Box::from_raw(start) });
        })
}

/// Where a thread [`start_thread`] started enters ring 3: the call leaves
/// the stack as every function expects to find it.
#[unsafe(naked)]
extern "C" fn thread_entry() -> ! {
    core::arch::naked_asm!(
        "and rsp, -16",
        "call {run}",
        "ud2",
        run = sym run_thread,
    )
}

extern "C" fn run_thread(start: u64) -> ! {
    // SAFETY: start_thread made the box and handed it to this thread
    // alone.
    let work = *unsafe { Box::from_raw(start as *mut Work) };
    crate::end_thread(work())
}

/// The pages the heap maps the first time it is asked for room, 1 MiB;
/// the bits that say which of its blocks are taken lie in its last pages.
const HEAP_PAGES: u64 = 256;
const HEAP_BYTES: usize = 256 * 4096;
const BITS: usize = HEAP_BYTES / heap::BLOCK / 64;

/// The heap of a program: a [`Pool`] over memory from [`crate::map`],
/// which [`main!`] makes the global allocator.
pub struct Allocator {
    taken: AtomicBool,
    pool: UnsafeCell<Option<Pool<'static>>>,
}

// SAFETY: the pool is only reached while `taken` is held.
unsafe impl Sync for Allocator {}

impl Default for Allocator {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator {
    #[must_use]
    pub const fn new() -> Allocator {
        Allocator {
            taken: AtomicBool::new(false),
            pool: UnsafeCell::new(None),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Option<Pool<'static>>) -> R) -> R {
        while self.taken.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        // SAFETY: `taken` makes this the only reference.
        let result = f(unsafe { &mut *self.pool.get() });
        self.taken.store(false, Ordering::Release);
        result
    }
}

/// Maps the heap's memory and puts its bits at its end.
fn first_pool() -> Option<Pool<'static>> {
    let at = usize::try_from(crate::map(HEAP_PAGES).ok()?).ok()?;
    let size = HEAP_BYTES - BITS * 8;
    // SAFETY: the kernel just mapped these pages for this program alone,
    // zeroed and writable, and the pool never hands out the bytes where
    // the bits lie.
    let bits = unsafe { core::slice::from_raw_parts_mut((at + size) as *mut u64, BITS) };
    Some(Pool::new(at, size, bits))
}

// SAFETY: every allocation is room the pool took for it alone, in memory
// that stays mapped, aligned as the layout asks or refused.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with(|pool| {
            if pool.is_none() {
                *pool = first_pool();
            }
            pool.as_mut()
                .and_then(|p| p.alloc(layout))
                .map_or(core::ptr::null_mut(), |at| at as *mut u8)
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.with(|pool| {
            if let Some(p) = pool.as_mut() {
                p.free(ptr as usize, layout);
            }
        });
    }
}

/// Makes `main`, a `fn(Handle) -> u64`, the program's main function: the
/// kernel starts the program with a handle, whose start messages the
/// runtime reads first (decision record 0017), and it ends with the code
/// main returns. Also gives the program its panic handler, its heap, and the
/// notes that make its file a `.mexec` file.
#[macro_export]
macro_rules! main {
    ($main:path) => {
        #[cfg(not(test))]
        extern "C" fn __program_main(argument: u64) -> ! {
            $crate::exit($main($crate::start::begin($crate::Handle(argument))))
        }

        // The kernel enters with the stack pointer at the top of the
        // stack; the call leaves it as every function expects to find it.
        #[cfg(not(test))]
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        #[unsafe(naked)]
        extern "C" fn _start() -> ! {
            core::arch::naked_asm!(
                "and rsp, -16",
                "call {main}",
                "ud2",
                main = sym __program_main,
            )
        }

        #[cfg(not(test))]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::panicked(info)
        }

        #[cfg(not(test))]
        #[global_allocator]
        static __PROGRAM_HEAP: $crate::Allocator = $crate::Allocator::new();

        // The notes that make the program a .mexec file: the version of
        // the format, and the program's name, which is its crate's.
        core::arch::global_asm!(
            ".pushsection .note.meridian, \"a\", @note",
            ".balign 4",
            ".long 9",
            ".long 4",
            ".long {version_kind}",
            ".asciz \"Meridian\"",
            ".balign 4",
            ".long {version}",
            ".long 9",
            ".long 3f - 2f",
            ".long {name_kind}",
            ".asciz \"Meridian\"",
            ".balign 4",
            "2:",
            concat!(".ascii \"", env!("CARGO_CRATE_NAME"), "\""),
            "3:",
            ".balign 4",
            ".popsection",
            version_kind = const $crate::abi::NOTE_VERSION,
            version = const $crate::abi::MEXEC_VERSION,
            name_kind = const $crate::abi::NOTE_NAME,
        );
    };
}
