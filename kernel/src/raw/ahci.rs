//! An AHCI controller and the SATA disks on it, through registers mapped
//! uncached at [`kernel::AHCI_REGISTERS`]. Each port with a device gets a
//! frame of its own for the command list, what the device sends back and
//! one command table, and every command waits for its answer: there are
//! no interrupts yet.

use core::sync::atomic::{Ordering, fence};

use ahci::{Attached, Identity, ata, hba, port};
use bootinfo::HHDM_BASE;
use paging::Flags;

use super::memory::{self, Spin};

/// Where in a port's frame each part lies.
const COMMAND_LIST: u64 = 0;
const RECEIVED: u64 = 0x400;
const TABLE: u64 = 0x800;

/// How long a command or a port may take, in cycles of the time stamp
/// counter: about a second on a fast processor.
const PATIENCE: u64 = 4_000_000_000;

/// A port with a device on it, ready for commands.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    pub port: usize,
    pub attached: Attached,
    /// What a disk said about itself; none for other devices.
    pub identity: Option<Identity>,
    frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The drive is not there or is no disk.
    NoDisk,
    /// The sectors asked for are past the disk's end, or too many.
    OutOfRange,
    /// The device did not answer in time.
    Timeout,
    /// It answered with an error.
    Failed,
    /// No memory was left for the data.
    NoMemory,
}

static DRIVES: Spin<[Option<Drive>; 32]> = Spin::new([None; 32]);

fn register(offset: usize) -> u32 {
    // SAFETY: start mapped the controller's two pages of registers at
    // AHCI_REGISTERS, uncached, and every offset used lies within them.
    unsafe { core::ptr::read_volatile((kernel::AHCI_REGISTERS + offset as u64) as *const u32) }
}

fn set_register(offset: usize, value: u32) {
    // SAFETY: as for register; the registers written are those the
    // specification lets software write.
    unsafe {
        core::ptr::write_volatile((kernel::AHCI_REGISTERS + offset as u64) as *mut u32, value);
    };
}

fn wait(done: impl Fn() -> bool) -> bool {
    x86::raw::wait_for(PATIENCE, done)
}

/// Takes the controller whose registers lie at physical `registers`:
/// maps them, switches it to AHCI, and gets every port with a device on it
/// ready, asking each disk who it is. Returns how many drives it found.
pub fn start(registers: u64) -> usize {
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
    let mapped = (0..2).all(|page| {
        memory::map_page(
            kernel::AHCI_REGISTERS + page * 4096,
            (registers & !0xfff) + page * 4096,
            flags,
        )
        .is_ok()
    });
    if !mapped {
        return 0;
    }
    set_register(hba::CONTROL, register(hba::CONTROL) | hba::AHCI_ENABLE);
    let implemented = register(hba::PORTS);
    let mut found = 0;
    for n in (0..32).filter(|n| implemented & 1 << n != 0) {
        let base = port::base(n);
        let attached = ahci::attached(
            register(base + port::STATUS),
            register(base + port::SIGNATURE),
        );
        if attached == Attached::Nothing {
            continue;
        }
        let Some(frame) = prepare(n) else {
            continue;
        };
        let mut drive = Drive {
            port: n,
            attached,
            identity: None,
            frame,
        };
        if attached == Attached::Disk {
            drive.identity = identify(&drive);
        }
        DRIVES.lock()[n] = Some(drive);
        found += 1;
    }
    found
}

/// Stops port `n`, points it at a frame of its own for its commands and
/// what the device sends, and starts it again.
fn prepare(n: usize) -> Option<u64> {
    let base = port::base(n);
    let command = register(base + port::COMMAND);
    set_register(
        base + port::COMMAND,
        command & !(port::START | port::RECEIVE),
    );
    let idle = wait(|| register(base + port::COMMAND) & (port::RUNNING | port::RECEIVING) == 0);
    if !idle {
        return None;
    }
    let frame = memory::device_frame()?;
    let split = |address: u64| {
        let half = |bits: u64| u32::try_from(bits & 0xffff_ffff).unwrap_or(0);
        (half(address), half(address >> 32))
    };
    let (list, list_high) = split(frame + COMMAND_LIST);
    let (received, received_high) = split(frame + RECEIVED);
    set_register(base + port::COMMAND_LIST, list);
    set_register(base + port::COMMAND_LIST_HIGH, list_high);
    set_register(base + port::RECEIVED, received);
    set_register(base + port::RECEIVED_HIGH, received_high);
    set_register(base + port::ERRORS, u32::MAX);
    set_register(base + port::INTERRUPTS, u32::MAX);
    set_register(
        base + port::COMMAND,
        register(base + port::COMMAND) | port::RECEIVE,
    );
    let ready = wait(|| register(base + port::TASK_FILE) & (port::BUSY | port::DATA_REQUEST) == 0);
    if !ready {
        memory::give_back(frame);
        return None;
    }
    set_register(
        base + port::COMMAND,
        register(base + port::COMMAND) | port::START,
    );
    Some(frame)
}

/// Runs `fis` on `drive`, moving `bytes` between the device and the frame
/// at physical `data`, and waits for its answer.
fn issue(
    drive: &Drive,
    fis: &[u8; 20],
    write_to_device: bool,
    data: u64,
    bytes: u32,
) -> Result<(), Error> {
    let entry = ahci::prdt_entry(data, bytes).ok_or(Error::OutOfRange)?;
    let header = ahci::command_header(write_to_device, 1, drive.frame + TABLE);
    let at = |offset: u64| (HHDM_BASE + drive.frame + offset) as *mut u8;
    // SAFETY: the drive's frame is its own and no command runs on it now:
    // each waits for its answer before the next is built.
    unsafe {
        core::ptr::copy_nonoverlapping(header.as_ptr(), at(COMMAND_LIST), header.len());
        core::ptr::write_bytes(at(TABLE), 0, 0x90);
        core::ptr::copy_nonoverlapping(fis.as_ptr(), at(TABLE), fis.len());
        core::ptr::copy_nonoverlapping(entry.as_ptr(), at(TABLE + 0x80), entry.len());
    }
    fence(Ordering::SeqCst);
    let base = port::base(drive.port);
    set_register(base + port::INTERRUPTS, u32::MAX);
    set_register(base + port::ISSUE, 1);
    let answered = wait(|| {
        register(base + port::ISSUE) & 1 == 0
            || register(base + port::INTERRUPTS) & port::TASK_FILE_ERROR != 0
    });
    fence(Ordering::SeqCst);
    if !answered {
        return Err(Error::Timeout);
    }
    let failed = register(base + port::INTERRUPTS) & port::TASK_FILE_ERROR != 0
        || register(base + port::TASK_FILE) & port::FAILED != 0;
    if failed { Err(Error::Failed) } else { Ok(()) }
}

/// Asks the disk on `drive` who it is.
fn identify(drive: &Drive) -> Option<Identity> {
    let buffer = memory::device_frame()?;
    let fis = ahci::command_fis(ata::IDENTIFY, 0, 0)?;
    let answer = issue(drive, &fis, false, buffer, 512);
    let mut data = [0u8; 512];
    // SAFETY: a frame of the kernel's own the device just wrote 512 bytes
    // into and is done with.
    unsafe {
        core::ptr::copy_nonoverlapping((HHDM_BASE + buffer) as *const u8, data.as_mut_ptr(), 512);
    };
    memory::give_back(buffer);
    answer.ok().map(|()| Identity::read(&data))
}

/// The drives found, by port.
pub fn drives() -> [Option<Drive>; 32] {
    *DRIVES.lock()
}

/// Reads the sectors from `lba` on into `into`, which takes any whole
/// number of them, a frame's worth at a time, from the disk on port `n`.
///
/// # Errors
///
/// As for [`read_sectors`].
pub fn read(n: usize, lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let size = DRIVES
        .lock()
        .get(n)
        .copied()
        .flatten()
        .and_then(|d| d.identity)
        .ok_or(Error::NoDisk)?
        .sector_size;
    let per_chunk = 4096 / u64::from(size).max(1);
    for (i, chunk) in (0..).zip(into.chunks_mut(4096)) {
        read_sectors(n, lba + i * per_chunk, chunk)?;
    }
    Ok(())
}

/// Reads the sectors from `lba` on into `into`, which takes a whole number
/// of them, at most a frame's worth, from the disk on port `n`.
///
/// # Errors
///
/// When there is no disk there, the sectors are not all on it, it does not
/// answer in time or answers with an error, or no memory is left.
pub fn read_sectors(n: usize, lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let drive = DRIVES
        .lock()
        .get(n)
        .copied()
        .flatten()
        .ok_or(Error::NoDisk)?;
    let identity = drive.identity.ok_or(Error::NoDisk)?;
    let size = usize::try_from(identity.sector_size).map_err(|_| Error::OutOfRange)?;
    let whole = !into.is_empty() && into.len().is_multiple_of(size) && into.len() <= 4096;
    let count = u16::try_from(into.len() / size.max(1)).map_err(|_| Error::OutOfRange)?;
    if !whole
        || lba
            .checked_add(u64::from(count))
            .is_none_or(|end| end > identity.sectors)
    {
        return Err(Error::OutOfRange);
    }
    let fis = ahci::command_fis(ata::READ_DMA_EXT, lba, count).ok_or(Error::OutOfRange)?;
    let buffer = memory::device_frame().ok_or(Error::NoMemory)?;
    let bytes = u32::try_from(into.len()).map_err(|_| Error::OutOfRange)?;
    let answer = issue(&drive, &fis, false, buffer, bytes);
    // SAFETY: as in identify.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (HHDM_BASE + buffer) as *const u8,
            into.as_mut_ptr(),
            into.len(),
        );
    };
    memory::give_back(buffer);
    answer
}
