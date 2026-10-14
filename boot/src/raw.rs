//! Everything in the loader that needs `unsafe`: the entry point the
//! firmware calls, port I/O for the serial line, and the paths that stop
//! the machine.

use core::arch::asm;
use core::fmt;

use uefi::Status;
use uefi::raw::System;
use uefi::raw::table::SystemTable;

use boot::serial;

#[unsafe(no_mangle)]
extern "efiapi" fn efi_main(image: uefi::raw::Handle, table: *mut SystemTable) -> Status {
    let mut serial = Serial::com1();
    // SAFETY: the firmware calls the entry point with its own system table
    // while boot services are running.
    let Some(system) = (unsafe { System::from_entry(image, table) }) else {
        let _ = fmt::Write::write_str(
            &mut serial,
            "boot: the firmware's system table is not valid\n",
        );
        return Status::LOAD_ERROR;
    };
    crate::run(&system, &mut serial)
}

/// COM1, set up for 115200 baud. Only COM1 can be opened, so no caller can
/// point the port writes anywhere else.
pub struct Serial {
    _only_through_com1: (),
}

impl Serial {
    pub fn com1() -> Serial {
        if let Some(sequence) = serial::init_sequence(serial::BASE_BAUD) {
            for (offset, value) in sequence {
                outb(serial::COM1 + offset, value);
            }
        }
        Serial {
            _only_through_com1: (),
        }
    }

    fn send(byte: u8) {
        for _ in 0..serial::MAX_POLLS {
            if inb(serial::COM1 + serial::LINE_STATUS) & serial::TRANSMIT_EMPTY != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        outb(serial::COM1, byte);
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in serial::wire_bytes(s) {
            Self::send(byte);
        }
        Ok(())
    }
}

fn outb(port: u16, value: u8) {
    // SAFETY: only called with the COM1 registers. Writing them sets up or
    // feeds the UART and touches no memory.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: only called with the COM1 line status register, which reads
    // without side effects.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Stops this CPU for good.
#[cfg_attr(test, allow(dead_code))]
pub fn halt() -> ! {
    loop {
        // SAFETY: disabling interrupts and halting has no effect on memory.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Ends QEMU through its isa-debug-exit device on port 0xf4. QEMU exits
/// with `(value << 1) | 1`: 33 for a pass, 35 for a failure.
#[cfg(feature = "qemu-test")]
pub fn qemu_exit(passed: bool) -> ! {
    let value: u32 = if passed { 0x10 } else { 0x11 };
    // SAFETY: test images only run in QEMU, where sextant attaches the
    // isa-debug-exit device to this port.
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4_u16, in("eax") value, options(nomem, nostack, preserves_flags));
    }
    halt()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut serial = Serial::com1();
    let _ = fmt::Write::write_fmt(&mut serial, format_args!("boot: panic: {info}\n"));
    #[cfg(feature = "qemu-test")]
    qemu_exit(false);
    #[cfg(not(feature = "qemu-test"))]
    halt()
}
