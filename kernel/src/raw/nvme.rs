//! An NVM Express controller and its first namespace, through registers
//! mapped uncached at [`kernel::NVME_REGISTERS`]. One pair of admin queues
//! and one pair for reading, each queue a frame of its own, and every
//! command waits for its completion: there are no interrupts yet.

use core::sync::atomic::{Ordering, fence};

use bootinfo::HHDM_BASE;
use nvme::{Capabilities, Command, Completion, Controller, Namespace, State, register};

use super::memory::{self, Spin};

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

/// Reads the blocks from `lba` on of the first namespace into `into`,
/// which takes any whole number of them, a frame's worth at a time.
///
/// # Errors
///
/// When no controller was started, the blocks are not all on it, it does
/// not answer in time or answers with an error, or no memory is left.
pub fn read(lba: u64, into: &mut [u8]) -> Result<(), Error> {
    let mut guard = DRIVE.lock();
    let drive = guard.as_mut().ok_or(Error::NoDrive)?;
    let per_chunk = span(drive, lba, into.len())?;
    let buffer = memory::device_frame().ok_or(Error::NoMemory)?;
    let mut answer = Ok(());
    for (i, chunk) in (0..).zip(into.chunks_mut(4096)) {
        answer = transfer(drive, nvme::read, lba + i * per_chunk, chunk.len(), buffer);
        if answer.is_err() {
            break;
        }
        // SAFETY: as in start, for the blocks just read.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (HHDM_BASE + buffer) as *const u8,
                chunk.as_mut_ptr(),
                chunk.len(),
            );
        };
    }
    memory::give_back(buffer);
    answer
}

/// Writes `from`, which takes any whole number of blocks, to the first
/// namespace from `lba` on, a frame's worth at a time.
///
/// # Errors
///
/// As for [`read`].
// Only the test image writes yet, until files can be written.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn write(lba: u64, from: &[u8]) -> Result<(), Error> {
    let mut guard = DRIVE.lock();
    let drive = guard.as_mut().ok_or(Error::NoDrive)?;
    let per_chunk = span(drive, lba, from.len())?;
    let buffer = memory::device_frame().ok_or(Error::NoMemory)?;
    let mut answer = Ok(());
    for (i, chunk) in (0..).zip(from.chunks(4096)) {
        // SAFETY: the frame is the kernel's alone, in the direct map, and
        // a chunk is no longer than it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                (HHDM_BASE + buffer) as *mut u8,
                chunk.len(),
            );
        };
        answer = transfer(drive, nvme::write, lba + i * per_chunk, chunk.len(), buffer);
        if answer.is_err() {
            break;
        }
    }
    memory::give_back(buffer);
    answer
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

/// Sends the command `build` makes for the `len` bytes from `lba` on and
/// the frame `buffer`, and waits for its answer.
fn transfer(
    drive: &mut Drive,
    build: fn(u16, u32, u64, u32, u64) -> Option<nvme::Command>,
    lba: u64,
    len: usize,
    buffer: u64,
) -> Result<(), Error> {
    let size = usize::try_from(drive.namespace.block_size).map_err(|_| Error::OutOfRange)?;
    let blocks = u32::try_from(len / size.max(1)).map_err(|_| Error::OutOfRange)?;
    let id = drive.next;
    drive.next = drive.next.wrapping_add(1).max(1);
    let command = build(id, 1, lba, blocks, buffer).ok_or(Error::OutOfRange)?;
    submit(&drive.capabilities, &mut drive.io, &command, id)
}
