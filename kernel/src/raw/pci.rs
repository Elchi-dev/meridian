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
/// behind each bridge, each bus once.
pub fn each(mut found: impl FnMut(Address, Header)) {
    let mut seen = [0u64; 4];
    let mut buses = [0u8; 256];
    let mut waiting = 1;
    while waiting > 0 {
        waiting -= 1;
        let bus = buses[waiting];
        if seen[usize::from(bus / 64)] & 1 << (bus % 64) != 0 {
            continue;
        }
        seen[usize::from(bus / 64)] |= 1 << (bus % 64);
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
                if h.layout == 1 && h.secondary_bus != 0 && waiting < buses.len() {
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
