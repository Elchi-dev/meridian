//! An NVM Express controller and its first namespace, through registers
//! mapped uncached at [`kernel::NVME_REGISTERS`]. One pair of admin queues
//! and one pair for reading, each queue a frame of its own, and every
//! command waits for its completion: there are no interrupts yet.

use core::sync::atomic::{Ordering, fence};

use bootinfo::HHDM_BASE;
use nvme::{Capabilities, Command, Completion, Controller, Namespace, State, register};

use super::memory::{self, DeviceFrames, Spin};

/// How many entries each queue has: a submission queue of them fills
/// exactly a frame.
const ENTRIES: u16 = 64;

/// How long the controller or a command may take, in cycles of the time
/// stamp counter: about two seconds on a fast processor.
const PATIENCE: u64 = 8_000_000_000;

#[derive(Clone, Copy, Debug)]
struct Queue {
    id: u16,
    submissions: u64,
    completions: u64,
    tail: u16,
    head: u16,
    phase: bool,
}

impl Queue {
    fn new(id: u16) -> Option<Queue> {
        Some(Queue {
            id,
            submissions: memory::device_frame()?,
            completions: memory::device_frame()?,
            tail: 0,
            head: 0,
            phase: true,
        })
    }
}

/// A controller ready for commands.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    pub controller: Controller,
    pub namespace: Namespace,
    capabilities: Capabilities,
    admin: Queue,
    io: Queue,
    /// The id of the next command. Ids only have to differ among the
    /// commands of a queue still running, and each command here is waited
    /// for, so one count does for both queues.
    next: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No controller was started.
    NoDrive,
    /// The blocks asked for are past the namespace's end, or too many.
    OutOfRange,
    /// The controller did not answer in time.
    Timeout,
    /// It answered with this status.
    Failed(u16),
    /// No memory was left for the data.
    NoMemory,
}

static DRIVE: Spin<Option<Drive>> = Spin::new(None);

fn register32(offset: usize) -> u32 {
    // SAFETY: start mapped the controller's registers at NVME_REGISTERS,
    // uncached, and every offset used lies within them.
    unsafe { core::ptr::read_volatile((kernel::NVME_REGISTERS + offset as u64) as *const u32) }
}

fn set_register32(offset: usize, value: u32) {
    // SAFETY: as for register32, for registers the host may write.
    unsafe {
        core::ptr::write_volatile((kernel::NVME_REGISTERS + offset as u64) as *mut u32, value);
    };
}

fn set_register64(offset: usize, value: u64) {
    let (low, high) = nvme::halves(value);
    set_register32(offset, low);
    set_register32(offset + 4, high);
}

fn wait(done: impl Fn() -> bool) -> bool {
    x86::raw::wait_for(PATIENCE, done)
}

/// Runs `command` on `queue`, with the id `id`, and waits for its
/// completion.
fn submit(
    capabilities: &Capabilities,
    queue: &mut Queue,
    command: &Command,
    id: u16,
) -> Result<(), Error> {
    let slot = HHDM_BASE + queue.submissions + u64::from(queue.tail) * 64;
    // SAFETY: the queue's frame is its own, and the controller has taken
    // every command before this slot: each waits for its completion.
    unsafe { core::ptr::copy_nonoverlapping(command.as_ptr(), slot as *mut u8, 64) };
    queue.tail = (queue.tail + 1) % ENTRIES;
    fence(Ordering::SeqCst);
    set_register32(
        capabilities.doorbell(queue.id, false),
        u32::from(queue.tail),
    );
    let entry = HHDM_BASE + queue.completions + u64::from(queue.head) * 16;
    let read_entry = || {
        let mut bytes = [0u8; 16];
        // SAFETY: the completion queue's frame is its own, and the
        // controller only writes entries the host has not yet taken.
        unsafe { core::ptr::copy_nonoverlapping(entry as *const u8, bytes.as_mut_ptr(), 16) };
        Completion::read(&bytes)
    };
    let phase = queue.phase;
    if !wait(|| read_entry().phase == phase) {
        return Err(Error::Timeout);
    }
    fence(Ordering::SeqCst);
    let completion = read_entry();
    queue.head = (queue.head + 1) % ENTRIES;
    if queue.head == 0 {
        queue.phase = !queue.phase;
    }
    set_register32(capabilities.doorbell(queue.id, true), u32::from(queue.head));
    match completion.status {
        0 if completion.id == id => Ok(()),
        status => Err(Error::Failed(status)),
    }
}

/// Takes the controller whose registers lie at physical `registers`:
/// maps them, turns it off and on again with queues of its own, asks it
/// and its first namespace who they are, and makes a pair of queues for
/// reading. Returns it once it is ready.
pub fn start(registers: u64) -> Option<Drive> {
    if !memory::map_registers(kernel::NVME_REGISTERS, registers & !0xfff, 2) {
        return None;
    }
    let low = u64::from(register32(register::CAPABILITIES));
    let high = u64::from(register32(register::CAPABILITIES + 4));
    let capabilities = Capabilities::read(high << 32 | low);
    if !capabilities.usable(ENTRIES, 2 * 4096) {
        return None;
    }
    set_register32(register::CONFIGURATION, 0);
    if !wait(|| register32(register::STATUS) & register::READY == 0) {
        return None;
    }
    let (Some(admin), Some(io)) = (Queue::new(0), Queue::new(1)) else {
        return None;
    };
    set_register32(register::ADMIN_QUEUES, nvme::admin_queues(ENTRIES));
    set_register64(register::ADMIN_SUBMISSION, admin.submissions);
    set_register64(register::ADMIN_COMPLETION, admin.completions);
    set_register32(register::CONFIGURATION, nvme::ENABLED);
    let state = || State::read(register32(register::STATUS));
    if !wait(|| state() != State::Off) || state() != State::Ready {
        return None;
    }
    let mut drive = Drive {
        controller: Controller::read(&[0; 4096]),
        namespace: Namespace {
            blocks: 0,
            block_size: 0,
        },
        capabilities,
        admin,
        io,
        next: 1,
    };
    let page = memory::device_frame()?;
    let mut data = [0u8; 4096];
    let mut identify = |drive: &mut Drive, namespace: u32| {
        let id = drive.next;
        drive.next += 1;
        let asked = submit(
            &drive.capabilities,
            &mut drive.admin,
            &nvme::identify(id, namespace, page),
            id,
        );
        // SAFETY: a frame of the kernel's own the controller just wrote its
        // answer into and is done with.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (HHDM_BASE + page) as *const u8,
                data.as_mut_ptr(),
                4096,
            );
        };
        asked.map(|()| data)
    };
    let controller = identify(&mut drive, 0).map(|d| Controller::read(&d));
    let namespace = identify(&mut drive, 1)
        .ok()
        .and_then(|d| Namespace::read(&d));
    memory::give_back(page);
    let (Ok(controller), Some(namespace)) = (controller, namespace) else {
        return None;
    };
    drive.controller = controller;
    drive.namespace = namespace;
    let queues = [
        nvme::create_completion_queue(drive.next, 1, ENTRIES, drive.io.completions),
        nvme::create_submission_queue(drive.next + 1, 1, ENTRIES, drive.io.submissions, 1),
    ];
    for (i, command) in (0..).zip(queues.iter()) {
        if submit(
            &drive.capabilities,
            &mut drive.admin,
            command,
            drive.next + i,
        )
        .is_err()
        {
            return None;
        }
    }
    *DRIVE.lock() = Some(drive);
    Some(drive)
}

/// The controller started, if any. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn drive() -> Option<Drive> {
    *DRIVE.lock()
}

/// The frames one command of `drive` moves the `len` bytes through, as
/// many as it takes, and a frame for the PRP entries of those past the
/// second.
fn take(drive: &Drive, len: usize) -> Option<(DeviceFrames, DeviceFrames)> {
    let most = drive
        .controller
        .transfer_pages
        .and_then(|pages| usize::try_from(pages).ok())
        .map_or(DeviceFrames::MOST, |pages| pages.min(DeviceFrames::MOST));
    Some((
        DeviceFrames::take(len.div_ceil(4096).min(most))?,
        DeviceFrames::take(1)?,
    ))
}

/// Reads the blocks from `lba` on of the first namespace into `into`,
/// which takes any whole number of them, as many frames at a time as the
/// controller takes, up to sixteen.
///
/// # Errors
///
/// When no controller was started, the blocks are not all on it, it does
/// not answer in time or answers with an error, or no memory is left.
pub fn read(lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let mut guard = DRIVE.lock();
    let drive = guard.as_mut().ok_or(Error::NoDrive)?;
    let per_frame = span(drive, lba, into.len())?;
    let (frames, list) = take(drive, into.len()).ok_or(Error::NoMemory)?;
    let per_piece = per_frame * frames.frames().len() as u64;
    for (i, piece) in (0..).zip(into.chunks_mut(frames.bytes())) {
        let at = lba + i * per_piece;
        transfer(drive, nvme::read, at, piece.len(), &frames, &list)?;
        for (frame, part) in frames.frames().iter().zip(piece.chunks_mut(4096)) {
            // SAFETY: as in start, for the blocks just read.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (HHDM_BASE + frame) as *const u8,
                    part.as_mut_ptr(),
                    part.len(),
                );
            };
        }
    }
    Ok(())
}

/// Writes `from`, which takes any whole number of blocks, to the first
/// namespace from `lba` on, as many frames at a time as [`read`] reads.
///
/// # Errors
///
/// As for [`read`].
// Only the test image writes yet, until files can be written.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn write(lba: u64, from: &[u8]) -> Result<(), Error> {
    let mut guard = DRIVE.lock();
    let drive = guard.as_mut().ok_or(Error::NoDrive)?;
    let per_frame = span(drive, lba, from.len())?;
    let (frames, list) = take(drive, from.len()).ok_or(Error::NoMemory)?;
    let per_piece = per_frame * frames.frames().len() as u64;
    for (i, piece) in (0..).zip(from.chunks(frames.bytes())) {
        for (frame, part) in frames.frames().iter().zip(piece.chunks(4096)) {
            // SAFETY: the frame is the kernel's alone, in the direct map,
            // and a part is no longer than it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    part.as_ptr(),
                    (HHDM_BASE + frame) as *mut u8,
                    part.len(),
                );
            };
        }
        let at = lba + i * per_piece;
        transfer(drive, nvme::write, at, piece.len(), &frames, &list)?;
    }
    Ok(())
}

/// How many blocks a frame holds, once the `len` bytes from `lba` on are
/// found to be a whole number of blocks of the namespace, all on it.
fn span(drive: &Drive, lba: u64, len: usize) -> Result<u64, Error> {
    let size = usize::try_from(drive.namespace.block_size).map_err(|_| Error::OutOfRange)?;
    let count = u64::try_from(len / size.max(1)).map_err(|_| Error::OutOfRange)?;
    let whole = len > 0 && len.is_multiple_of(size);
    if !whole
        || lba
            .checked_add(count)
            .is_none_or(|end| end > drive.namespace.blocks)
    {
        return Err(Error::OutOfRange);
    }
    u64::try_from(4096 / size).map_err(|_| Error::OutOfRange)
}

/// What builds a command that moves blocks through the pages two PRP
/// entries name: [`nvme::read`] or [`nvme::write`].
type Build = fn(u16, u32, u64, u32, (u64, u64)) -> Option<Command>;

/// Sends the command `build` makes for the `len` bytes from `lba` on,
/// which fill the first of `frames`, with `list` for the PRP entries, and
/// waits for its answer.
fn transfer(
    drive: &mut Drive,
    build: Build,
    lba: u64,
    len: usize,
    frames: &DeviceFrames,
    list: &DeviceFrames,
) -> Result<(), Error> {
    let size = usize::try_from(drive.namespace.block_size).map_err(|_| Error::OutOfRange)?;
    let blocks = u32::try_from(len / size.max(1)).map_err(|_| Error::OutOfRange)?;
    let pages = frames.frames().get(..len.div_ceil(4096));
    let &[list] = list.frames() else {
        return Err(Error::NoMemory);
    };
    // SAFETY: the list's frame is the kernel's alone and in the direct
    // map; the controller reads it only while the command below runs.
    let entries = unsafe { &mut *((HHDM_BASE + list) as *mut [u8; 4096]) };
    let prp = pages
        .and_then(|pages| nvme::prps(pages, list, entries))
        .ok_or(Error::OutOfRange)?;
    let id = drive.next;
    drive.next = drive.next.wrapping_add(1).max(1);
    let command = build(id, 1, lba, blocks, prp).ok_or(Error::OutOfRange)?;
    submit(&drive.capabilities, &mut drive.io, &command, id)
}

/// How many commands went to the controller so far, on both queues, as
/// their ids count them. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn commands() -> u16 {
    DRIVE.lock().as_ref().map_or(0, |d| d.next)
}
