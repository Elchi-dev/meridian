//! An AHCI controller and the SATA disks on it, through registers mapped
//! uncached at [`kernel::AHCI_REGISTERS`]. Each port with a device gets a
//! frame of its own for the command list, what the device sends back and
//! one command table, and every command waits for its answer: there are
//! no interrupts yet.

use core::sync::atomic::{Ordering, fence};

use ahci::{Attached, Identity, ata, hba, port};
use bootinfo::HHDM_BASE;

use super::memory::{self, DeviceFrames, Spin};

/// Where in a port's frame each part lies.
const COMMAND_LIST: u64 = 0;
const RECEIVED: u64 = 0x400;
const TABLE: u64 = 0x800;

/// How long a command or a port may take, in cycles of the time stamp
/// counter: about a second on a fast processor.
const PATIENCE: u64 = 4_000_000_000;

/// The most bytes one command moves: as many as a [`DeviceFrames`] holds.
const MOST: usize = DeviceFrames::MOST * 4096;

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
    if !memory::map_registers(kernel::AHCI_REGISTERS, registers & !0xfff, 2) {
        return 0;
    }
    set_register(hba::CONTROL, register(hba::CONTROL) | hba::AHCI_ENABLE);
    let mut found = 0;
    for n in ahci::ports(register(hba::PORTS)) {
        let base = port::base(n);
        let attached = ahci::attached(
            register(base + port::STATUS),
            register(base + port::SIGNATURE),
        );
        if attached == Attached::Nothing {
            continue;
        }
        let Some(frame) = memory::device_frame() else {
            continue;
        };
        if !start_port(n, frame) {
            memory::give_back(frame);
            continue;
        }
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

/// Writes `address` into the register at `offset` and the one after it.
fn set_address(offset: usize, address: u64) {
    let (low, high) = ahci::halves(address);
    set_register(offset, low);
    set_register(offset + 4, high);
}

/// Stops port `n`, points it at `frame` for its commands and what the
/// device sends, clears the errors it noted, and starts it again: what a
/// port needs before its first command, and after one failed.
fn start_port(n: usize, frame: u64) -> bool {
    let base = port::base(n);
    let command = register(base + port::COMMAND);
    set_register(
        base + port::COMMAND,
        command & !(port::START | port::RECEIVE),
    );
    let idle = wait(|| register(base + port::COMMAND) & (port::RUNNING | port::RECEIVING) == 0);
    if !idle {
        return false;
    }
    set_address(base + port::COMMAND_LIST, frame + COMMAND_LIST);
    set_address(base + port::RECEIVED, frame + RECEIVED);
    set_register(base + port::ERRORS, u32::MAX);
    set_register(
        base + port::COMMAND,
        register(base + port::COMMAND) | port::RECEIVE,
    );
    let ready = wait(|| register(base + port::TASK_FILE) & (port::BUSY | port::DATA_REQUEST) == 0);
    if ready {
        set_register(
            base + port::COMMAND,
            register(base + port::COMMAND) | port::START,
        );
    }
    ready
}

/// The command header the last command on port `n` went with, which told
/// the controller among others which way its data went. QEMU goes by the
/// command instead, real controllers by the header. Only the test image
/// asks.
#[cfg(feature = "qemu-test")]
pub fn last_header(n: usize) -> Option<[u8; 32]> {
    let frame = DRIVES.lock().get(n)?.as_ref()?.frame;
    let mut header = [0u8; 32];
    // SAFETY: the drive's frame is its own, in the direct map, and its
    // command list starts it; reading it changes nothing.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (HHDM_BASE + frame + COMMAND_LIST) as *const u8,
            header.as_mut_ptr(),
            header.len(),
        );
    }
    Some(header)
}

/// Runs `fis` on `drive`, moving `bytes` between the device and the first
/// of `frames`, and waits for its answer.
fn issue(
    drive: &Drive,
    fis: &[u8; 20],
    write_to_device: bool,
    frames: &DeviceFrames,
    bytes: usize,
) -> Result<(), Error> {
    let bytes = u32::try_from(bytes).map_err(|_| Error::OutOfRange)?;
    let mut entries = [[0u8; 16]; DeviceFrames::MOST];
    let count = ahci::prdt(frames.frames(), bytes, &mut entries).ok_or(Error::OutOfRange)?;
    let header = ahci::command_header(write_to_device, count, drive.frame + TABLE);
    let at = |offset: u64| (HHDM_BASE + drive.frame + offset) as *mut u8;
    // SAFETY: the drive's frame is its own and no command runs on it now:
    // each waits for its answer before the next is built. The table with
    // all its entries ends well inside the frame.
    unsafe {
        core::ptr::copy_nonoverlapping(header.as_ptr(), at(COMMAND_LIST), header.len());
        core::ptr::write_bytes(at(TABLE), 0, 0x80 + 16 * DeviceFrames::MOST);
        core::ptr::copy_nonoverlapping(fis.as_ptr(), at(TABLE), fis.len());
        core::ptr::copy_nonoverlapping(
            entries.as_ptr().cast::<u8>(),
            at(TABLE + 0x80),
            16 * usize::from(count),
        );
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
    let failed = register(base + port::INTERRUPTS) & port::TASK_FILE_ERROR != 0;
    if answered && !failed {
        return Ok(());
    }
    // A port stops taking commands after an error, and one that never
    // answered still has its command; starting it again clears both. One
    // that cannot be started again is no drive any more.
    if !start_port(drive.port, drive.frame) {
        DRIVES.lock()[drive.port] = None;
    }
    Err(if answered {
        Error::Failed
    } else {
        Error::Timeout
    })
}

/// Asks the disk on `drive` who it is.
fn identify(drive: &Drive) -> Option<Identity> {
    let frames = DeviceFrames::take(1)?;
    let fis = ahci::command_fis(ata::IDENTIFY, 0, 0)?;
    issue(drive, &fis, false, &frames, 512).ok()?;
    let mut data = [0u8; 512];
    // SAFETY: a frame of the kernel's own the device just wrote 512 bytes
    // into and is done with.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (HHDM_BASE + frames.frames()[0]) as *const u8,
            data.as_mut_ptr(),
            512,
        );
    };
    Some(Identity::read(&data))
}

/// Sends `command` to the disk on port `n`, with a sector's room for what
/// it sends back. Only the test image does, to see one fail.
///
/// # Errors
///
/// As for [`read_sectors`].
#[cfg(feature = "qemu-test")]
pub fn command(n: usize, command: u8) -> Result<(), Error> {
    let drive = DRIVES
        .lock()
        .get(n)
        .copied()
        .flatten()
        .ok_or(Error::NoDisk)?;
    let fis = ahci::command_fis(command, 0, 0).ok_or(Error::OutOfRange)?;
    let frames = DeviceFrames::take(1).ok_or(Error::NoMemory)?;
    issue(&drive, &fis, false, &frames, 512)
}

/// The register FIS the device on port `n` last sent, where the port puts
/// it in the drive's frame. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn answer(n: usize) -> Option<[u8; 20]> {
    let drive = DRIVES.lock().get(n).copied().flatten()?;
    let mut fis = [0u8; 20];
    // SAFETY: the drive's frame is its own, and the port is done writing
    // the answer to the last command into it.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (HHDM_BASE + drive.frame + RECEIVED + 0x40) as *const u8,
            fis.as_mut_ptr(),
            fis.len(),
        );
    };
    Some(fis)
}

/// The register at `offset` of port `n`. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn port_register(n: usize, offset: usize) -> u32 {
    register(port::base(n) + offset)
}

/// Sets the register at `offset` of port `n`. Only the test image does, to
/// leave a port as a firmware might.
#[cfg(feature = "qemu-test")]
pub fn set_port_register(n: usize, offset: usize, value: u32) {
    set_register(port::base(n) + offset, value);
}

/// Starts port `n` again on its drive's frame. Only the test image does,
/// once it left the port as a firmware might.
#[cfg(feature = "qemu-test")]
pub fn restart(n: usize) -> bool {
    let drive = DRIVES.lock().get(n).copied().flatten();
    drive.is_some_and(|d| start_port(n, d.frame))
}

/// The drives found, by port.
pub fn drives() -> [Option<Drive>; 32] {
    *DRIVES.lock()
}

/// Reads the sectors from `lba` on into `into`, which takes any whole
/// number of them, sixteen frames at a time, from the disk on port `n`.
///
/// # Errors
///
/// As for [`read_sectors`].
pub fn read(n: usize, lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let per_command = (MOST / sector_size(n)?.max(1)) as u64;
    for (i, chunk) in (0..).zip(into.chunks_mut(MOST)) {
        read_sectors(n, lba + i * per_command, chunk)?;
    }
    Ok(())
}

/// Reads the sectors from `lba` on into `into`, which takes a whole number
/// of them, at most sixteen frames' worth, from the disk on port `n`, with
/// one command.
///
/// # Errors
///
/// When there is no disk there, the sectors are not all on it, it does not
/// answer in time or answers with an error, or no memory is left.
pub fn read_sectors(n: usize, lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let (drive, fis) = span(n, ata::READ_DMA_EXT, lba, into.len())?;
    let frames = DeviceFrames::take(into.len().div_ceil(4096)).ok_or(Error::NoMemory)?;
    issue(&drive, &fis, false, &frames, into.len())?;
    for (frame, part) in frames.frames().iter().zip(into.chunks_mut(4096)) {
        // SAFETY: as in identify.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (HHDM_BASE + frame) as *const u8,
                part.as_mut_ptr(),
                part.len(),
            );
        };
    }
    Ok(())
}

/// Writes `from`, a whole number of sectors and at most sixteen frames'
/// worth, to the disk on port `n` from `lba` on, with one command.
///
/// # Errors
///
/// As for [`read_sectors`].
// Only the test image writes yet, until files can be written.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn write_sectors(n: usize, lba: u64, from: &[u8]) -> Result<(), Error> {
    let (drive, fis) = span(n, ata::WRITE_DMA_EXT, lba, from.len())?;
    let frames = DeviceFrames::take(from.len().div_ceil(4096)).ok_or(Error::NoMemory)?;
    for (frame, part) in frames.frames().iter().zip(from.chunks(4096)) {
        // SAFETY: the frame is the kernel's alone, in the direct map, and
        // a part is no longer than it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                part.as_ptr(),
                (HHDM_BASE + frame) as *mut u8,
                part.len(),
            );
        };
    }
    issue(&drive, &fis, true, &frames, from.len())
}

/// Writes `from`, which takes any whole number of sectors, to the disk on
/// port `n` from `lba` on, sixteen frames at a time.
///
/// # Errors
///
/// As for [`read_sectors`].
// Only the test image writes yet, until files can be written.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn write(n: usize, lba: u64, from: &[u8]) -> Result<(), Error> {
    let per_command = (MOST / sector_size(n)?.max(1)) as u64;
    for (i, chunk) in (0..).zip(from.chunks(MOST)) {
        write_sectors(n, lba + i * per_command, chunk)?;
    }
    Ok(())
}

/// The size of the sectors of the disk on port `n`.
fn sector_size(n: usize) -> Result<usize, Error> {
    let size = DRIVES
        .lock()
        .get(n)
        .copied()
        .flatten()
        .and_then(|d| d.identity)
        .ok_or(Error::NoDisk)?
        .sector_size;
    usize::try_from(size).map_err(|_| Error::OutOfRange)
}

/// The drive on port `n` and the FIS that carries `command` for the sectors
/// from `lba` on that `len` bytes, a whole number of sectors and at most
/// [`MOST`], take; an error when they are not all on its disk.
fn span(n: usize, command: u8, lba: u64, len: usize) -> Result<(Drive, [u8; 20]), Error> {
    let drive = DRIVES
        .lock()
        .get(n)
        .copied()
        .flatten()
        .ok_or(Error::NoDisk)?;
    let identity = drive.identity.ok_or(Error::NoDisk)?;
    let size = usize::try_from(identity.sector_size).map_err(|_| Error::OutOfRange)?;
    let whole = len > 0 && len.is_multiple_of(size) && len <= MOST;
    let count = u16::try_from(len / size.max(1)).map_err(|_| Error::OutOfRange)?;
    if !whole
        || lba
            .checked_add(u64::from(count))
            .is_none_or(|end| end > identity.sectors)
    {
        return Err(Error::OutOfRange);
    }
    let fis = ahci::command_fis(command, lba, count).ok_or(Error::OutOfRange)?;
    Ok((drive, fis))
}
