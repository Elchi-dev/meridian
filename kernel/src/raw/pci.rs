//! PCI configuration space through the ECAM region the MCFG names, mapped
//! uncached at [`kernel::PCI_CONFIG`] a page at a time as functions are
//! read, and walked from bus 0 through every bridge.

use acpi::Ecam;
use paging::Flags;
use pci::{Address, Header};

use super::memory::Spin;

static REGION: Spin<Option<Ecam>> = Spin::new(None);

/// Remembers where configuration space lies, for [`header`] and [`each`].
pub fn start(ecam: Option<Ecam>) {
    *REGION.lock() = ecam;
}

/// Where the function's 4 KiB of configuration space are mapped, mapping
/// them first; none outside the region or when no page could be mapped.
fn config(at: Address) -> Option<u64> {
    let ecam = (*REGION.lock())?;
    if at.bus > ecam.last_bus {
        return None;
    }
    let offset = at.ecam_offset(ecam.first_bus)?;
    let virt = kernel::PCI_CONFIG + offset;
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
    match super::memory::map_page(virt, ecam.base + offset, flags) {
        Ok(()) | Err(paging::Error::AlreadyMapped(_)) => Some(virt),
        Err(_) => None,
    }
}

/// Lets the function at `at` answer at the memory its registers are mapped
/// to and read and write memory itself, as a controller that moves data
/// does: bits 1 and 2 of its command register.
pub fn enable(at: Address) {
    let Some(virt) = config(at) else {
        return;
    };
    let register = (virt + 4) as *mut u32;
    // SAFETY: config mapped the function's configuration space; the upper
    // half written as 0 leaves its status bits, which clear on a 1, alone.
    unsafe {
        let command = core::ptr::read_volatile(register) & 0xffff;
        core::ptr::write_volatile(register, command | 0x6);
    }
}

/// The vendor id configuration space holds for `at`: [`pci::NOBODY`] where
/// no function is, none outside the region. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn vendor(at: Address) -> Option<u16> {
    let virt = config(at)?;
    // SAFETY: as in header.
    let word = unsafe { core::ptr::read_volatile(virt as *const u32) };
    u16::try_from(word & 0xffff).ok()
}

/// The command register of the function at `at`. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn command(at: Address) -> Option<u16> {
    let virt = config(at)?;
    // SAFETY: as in enable, reading only.
    let word = unsafe { core::ptr::read_volatile((virt + 4) as *const u32) };
    u16::try_from(word & 0xffff).ok()
}

/// Sets the command register of the function at `at`. Only the test image
/// does, to see [`enable`] turn its bits on.
#[cfg(feature = "qemu-test")]
pub fn set_command(at: Address, command: u16) {
    if let Some(virt) = config(at) {
        // SAFETY: as in enable.
        unsafe { core::ptr::write_volatile((virt + 4) as *mut u32, u32::from(command)) };
    }
}

/// The header of the function at `at`, or none where no function is.
pub fn header(at: Address) -> Option<Header> {
    let virt = config(at)?;
    let mut bytes = [0u8; 64];
    for (i, word) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        // SAFETY: config mapped the function's configuration space,
        // uncached, and reading it in 32-bit words changes nothing.
        let value = unsafe { core::ptr::read_volatile((virt + 4 * i as u64) as *const u32) };
        *word = value.to_le_bytes();
    }
    Header::read(&bytes)
}

/// Hands every function there is to `found`: those on bus 0, then those
/// behind each bridge. A bridge's bus counts only when it is above the
/// bridge's own, as PCI numbers them, so no bus is reached in a circle.
pub fn each(mut found: impl FnMut(Address, Header)) {
    let mut buses = [0u8; 256];
    let mut waiting = 1;
    while waiting > 0 {
        waiting -= 1;
        let bus = buses[waiting];
        for device in 0..32 {
            for function in 0..8 {
                let at = Address {
                    bus,
                    device,
                    function,
                };
                let Some(h) = header(at) else {
                    if function == 0 {
                        break;
                    }
                    continue;
                };
                found(at, h);
                if h.layout == 1 && h.secondary_bus > bus && waiting < buses.len() {
                    buses[waiting] = h.secondary_bus;
                    waiting += 1;
                }
                if function == 0 && !h.multifunction {
                    break;
                }
            }
        }
    }
}
