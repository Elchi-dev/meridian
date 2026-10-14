//! Everything in the loader that needs `unsafe`: the entry point the
//! firmware calls, the page table frames, the jump into the kernel and
//! the panic handler. The serial port and the instructions that stop the
//! machine are shared with the kernel in `lib/x86`.

use core::arch::asm;
use core::fmt;

use core::mem::size_of;

use boot::handover::{self, Pool, Problem};
use boot::memory;
use bootinfo::{BootInfo, HHDM_BASE, KERNEL_MEMORY_TYPE, MemoryRegion, RegionKind, STACK_TOP};
use paging::Frames;
use uefi::Status;
use uefi::memory::MemoryType;
use uefi::raw::System;
use uefi::raw::table::SystemTable;
use x86::raw::Com1;

use crate::{REGION_LIMIT, Ready};

#[unsafe(no_mangle)]
extern "efiapi" fn efi_main(image: uefi::raw::Handle, table: *mut SystemTable) -> Status {
    let mut serial = Com1::open();
    // SAFETY: the firmware calls the entry point with its own system table
    // while boot services are running.
    let Some(system) = (unsafe { System::from_entry(image, table) }) else {
        let _ = fmt::Write::write_str(
            &mut serial,
            "boot: the firmware's system table is not valid\n",
        );
        return Status::LOAD_ERROR;
    };
    #[cfg(feature = "qemu-test")]
    if crate::qemu_test::is_chained(&system) {
        let _ = fmt::Write::write_str(
            &mut serial,
            "boot: started by the loader as a test, returning\n",
        );
        return crate::qemu_test::CHAINED;
    }
    crate::run(system, &mut serial)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut serial = Com1::open();
    let _ = fmt::Write::write_fmt(&mut serial, format_args!("boot: panic: {info}\n"));
    #[cfg(feature = "qemu-test")]
    x86::raw::qemu_exit(false);
    #[cfg(not(feature = "qemu-test"))]
    x86::raw::halt()
}

/// Reads the final memory map into the boot information, leaves boot
/// services and jumps. Only comes back when the firmware will not let go.
pub fn start(mut system: System, ready: Ready) -> (System, Problem) {
    const EMPTY: MemoryRegion = MemoryRegion {
        start: 0,
        pages: 0,
        kind: RegionKind::RESERVED,
        reserved: 0,
    };
    let Ready {
        root,
        entry,
        mut info,
        page,
        page_at,
        map: map_buf,
    } = ready;
    let mut regions = [EMPTY; REGION_LIMIT];
    let mut refused = Status::ABORTED;
    // The map can change once between reading it and leaving, when a
    // firmware event fires; a second refusal is a real problem.
    for _ in 0..2 {
        let map = match system.memory_map(map_buf) {
            Ok(map) => map,
            Err(status) => return (system, crate::firmware("reading the memory map")(status)),
        };
        let count = match memory::convert(map.descriptors(), &mut regions) {
            Ok(count) => count,
            Err(too_small) => return (system, Problem::Map(too_small)),
        };
        info.memory = bootinfo::MemoryMap {
            regions: HHDM_BASE + page_at + size_of::<BootInfo>() as u64,
            count: count as u64,
        };
        handover::write_info(page, &info, &regions[..count]);
        match system.exit_boot_services(map.key) {
            Ok(()) => {
                let _ = x86::raw::enable_no_execute();
                x86::raw::enable_write_protect();
                // SAFETY: boot services are over, and handover::build
                // mapped this loader where it runs, the stack, the kernel
                // and all memory, which holds the boot information.
                unsafe { enter_kernel(root, STACK_TOP, entry, HHDM_BASE + page_at) }
            }
            Err((back, status)) => {
                system = back;
                refused = status;
            }
        }
    }
    (
        system,
        Problem::Firmware {
            doing: "leaving boot services",
            status: refused,
        },
    )
}

/// Page table frames from one block of firmware pages, allocated as kernel
/// memory so the kernel finds them marked as its own. Boot services keep
/// memory identity mapped, so a frame's address is also where it is.
pub struct FirmwareFrames {
    pool: Pool,
}

impl FirmwareFrames {
    /// Allocates `count` cleared frames.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status when it has no room.
    pub fn new(system: &System, count: usize) -> Result<FirmwareFrames, Status> {
        let pages = system.allocate_pages(MemoryType(KERNEL_MEMORY_TYPE), count)?;
        Ok(FirmwareFrames {
            pool: Pool::new(pages.start(), count as u64),
        })
    }
}

impl Frames for FirmwareFrames {
    fn allocate(&mut self) -> Option<u64> {
        self.pool.take()
    }

    fn table(&mut self, physical: u64) -> &mut [u64; 512] {
        assert!(
            self.pool.handed_out(physical),
            "{physical:#x} is not a page table frame of this loader"
        );
        // SAFETY: the frame was allocated from the firmware for this pool
        // alone, is identity mapped while boot services run, and the
        // returned borrow ends before the next call can make another.
        unsafe { &mut *(physical as *mut [u64; 512]) }
    }
}

/// Switches to the kernel's page tables and stack and calls its entry
/// point with the boot information in `rdi`. Interrupts go off first, since
/// nothing in the new address space could take them.
///
/// # Safety
///
/// `root` must map this function at the address it runs from, the stack
/// below `stack_top`, the kernel's entry point and the boot information,
/// and boot services must be over.
pub unsafe fn enter_kernel(root: u64, stack_top: u64, entry: u64, info: u64) -> ! {
    // SAFETY: the caller promises the new address space holds everything
    // the next instructions touch. The kernel never returns; if it did,
    // the loop below stops this processor.
    unsafe {
        asm!(
            "cli",
            "mov cr3, {root}",
            "mov rsp, {stack}",
            "xor ebp, ebp",
            "call {entry}",
            "2:",
            "hlt",
            "jmp 2b",
            root = in(reg) root,
            stack = in(reg) stack_top,
            entry = in(reg) entry,
            in("rdi") info,
            options(noreturn),
        );
    }
}
