//! Everything in the loader that needs `unsafe`: the entry point the
//! firmware calls and the panic handler. The serial port and the
//! instructions that stop the machine are shared with the kernel in
//! `lib/x86`.

use core::fmt;

use uefi::Status;
use uefi::raw::System;
use uefi::raw::table::SystemTable;
use x86::raw::Com1;

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
    crate::run(&system, &mut serial)
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
