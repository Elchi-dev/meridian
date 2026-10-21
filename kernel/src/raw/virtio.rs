//! A virtio block device, through the structures its capabilities name:
//! reset, the features agreed, one queue of a few entries in a frame of
//! its own, and reads of up to a frame each, one at a time, polled.

use core::sync::atomic::{Ordering, fence};

use bootinfo::HHDM_BASE;
use pci::{Address, Bar, Header};
use virtio::{Region, block, common, status};

use super::memory::{self, Spin};

/// Where the common structure, the device's and the queue's notification
/// address are mapped, a page each.
const COMMON: u64 = kernel::VIRTIO_REGISTERS;
const DEVICE: u64 = kernel::VIRTIO_REGISTERS + 0x1000;
const NOTIFY: u64 = kernel::VIRTIO_REGISTERS + 0x2000;

/// How long the device may take to answer, in cycles of the time stamp
/// counter: about a second on a fast processor. A count of looks instead
/// ran out within a millisecond on the queue's ring, which lies in memory,
/// whenever QEMU was slow to get to the request.
const PATIENCE: u64 = 4_000_000_000;

/// Where the request's header and status byte lie in the queue's frame,
/// past its rings.
const HEADER: u64 = 0x800;
const STATUS: u64 = 0x810;

/// What a block device the kernel drives is like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drive {
    /// Its size, in sectors of 512 bytes.
    pub sectors: u64,
    /// Whether it says it only reads.
    pub read_only: bool,
}

struct Queue {
    drive: Drive,
    /// The frame of the queue: its descriptors and rings, and the request.
    frame: u64,
    size: u16,
    /// Where the queue's notification address is mapped.
    notify: u64,
    /// How many requests went to the device, and how many it answered.
    sent: u16,
}

static QUEUE: Spin<Option<Queue>> = Spin::new(None);

/// Why a read did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NoDrive,
    /// Not a whole number of sectors, or not all on the device.
    OutOfRange,
    NoMemory,
    /// The device did not answer in time.
    TimedOut,
    /// It answered with this status.
    Failed(u8),
    /// It only reads.
    // Only the test image writes yet, until files can be written.
    #[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
    ReadOnly,
}

fn read8(at: u64) -> u8 {
    // SAFETY: every caller passes an address in a page start mapped for
    // the device's structures, uncached, where reading changes nothing.
    unsafe { core::ptr::read_volatile(at as *const u8) }
}

fn read16(at: u64) -> u16 {
    // SAFETY: as for read8, at a register that is 16 bits wide.
    unsafe { core::ptr::read_volatile(at as *const u16) }
}

fn read32(at: u64) -> u32 {
    // SAFETY: as for read8, at a register that is 32 bits wide.
    unsafe { core::ptr::read_volatile(at as *const u32) }
}

fn write8(at: u64, value: u8) {
    // SAFETY: as for read8; the register takes what the driver writes as
    // the specification says it may.
    unsafe { core::ptr::write_volatile(at as *mut u8, value) };
}

fn write16(at: u64, value: u16) {
    // SAFETY: as for write8, at a register that is 16 bits wide.
    unsafe { core::ptr::write_volatile(at as *mut u16, value) };
}

fn write32(at: u64, value: u32) {
    // SAFETY: as for write8, at a register that is 32 bits wide.
    unsafe { core::ptr::write_volatile(at as *mut u32, value) };
}

/// `value` as its low and its high half.
fn halves(value: u64) -> (u32, u32) {
    let bytes = value.to_le_bytes();
    let (low, high) = bytes.split_at(4);
    let half = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    (half(low), half(high))
}

/// A 64-bit register, written as two halves, the low one first.
fn write64(at: u64, value: u64) {
    let (low, high) = halves(value);
    let [low_at, high_at] = virtio::halves(at);
    write32(low_at, low);
    write32(high_at, high);
}

/// Maps the page that holds `region` of the device whose header is `h`,
/// `skip` bytes further in, at `virt`, and answers where it starts there.
fn map(h: &Header, region: Region, skip: u64, virt: u64) -> Option<u64> {
    let Bar::Memory { address, .. } = h.bar(usize::from(region.bar)) else {
        return None;
    };
    let (page, at) = virtio::window(region.at(address, skip), virt);
    memory::map_registers(virt, page, 1).then_some(at)
}

/// Waits until `done` holds, for no longer than [`PATIENCE`].
fn wait(done: impl Fn() -> bool) -> bool {
    x86::raw::wait_for(PATIENCE, done)
}

/// Starts the block device at `at`, whose header is `h`: resets it, agrees
/// on the features, sets up its first queue and says it is ready. None
/// when it is no device of version 1 or does not come up.
pub fn start(at: Address, h: &Header) -> Option<Drive> {
    let layout = virtio::layout(&super::pci::config_bytes(at)?)?;
    let base = map(h, layout.common, 0, COMMON)?;
    let device = map(h, layout.device, 0, DEVICE)?;
    let status_at = base + common::DEVICE_STATUS;
    write8(status_at, 0);
    if !wait(|| read8(status_at) == 0) {
        return None;
    }
    write8(status_at, status::ACKNOWLEDGE | status::DRIVER);
    let half = |select: u32| {
        write32(base + common::DEVICE_FEATURE_SELECT, select);
        u64::from(read32(base + common::DEVICE_FEATURE))
    };
    let offered = half(0) | half(1) << 32;
    let Some(taken) = virtio::accept(offered) else {
        write8(status_at, status::FAILED);
        return None;
    };
    let (low, high) = halves(taken);
    write32(base + common::DRIVER_FEATURE_SELECT, 0);
    write32(base + common::DRIVER_FEATURE, low);
    write32(base + common::DRIVER_FEATURE_SELECT, 1);
    write32(base + common::DRIVER_FEATURE, high);
    write8(status_at, read8(status_at) | status::FEATURES_OK);
    if read8(status_at) & status::FEATURES_OK == 0 {
        write8(status_at, status::FAILED);
        return None;
    }
    let (frame, size, notify) = start_queue(base, h, &layout)?;
    write8(status_at, read8(status_at) | status::DRIVER_OK);
    let drive = Drive {
        sectors: capacity(base, device),
        read_only: virtio::read_only(taken),
    };
    *QUEUE.lock() = Some(Queue {
        drive,
        frame,
        size,
        notify,
        sent: 0,
    });
    Some(drive)
}

/// The device's capacity, read until its configuration did not change in
/// between.
fn capacity(base: u64, device: u64) -> u64 {
    loop {
        let before = read8(base + 0x15);
        let [low, high] = virtio::halves(device).map(read32);
        if read8(base + 0x15) == before {
            return x86::join(high, low);
        }
    }
}

/// Sets the first queue up in a frame of its own, as large as the device
/// and [`virtio::QUEUE_SIZE`] allow, and enables it. Answers the frame, the
/// size and where the queue is rung.
fn start_queue(base: u64, h: &Header, layout: &virtio::Layout) -> Option<(u64, u16, u64)> {
    write16(base + common::QUEUE_SELECT, 0);
    let size = virtio::queue_size(read16(base + common::QUEUE_SIZE))?;
    write16(base + common::QUEUE_SIZE, size);
    let frame = memory::device_frame()?;
    let rings = virtio::rings(size);
    write64(base + common::QUEUE_DESC, frame);
    write64(base + common::QUEUE_DRIVER, frame + rings.available as u64);
    write64(base + common::QUEUE_DEVICE, frame + rings.used as u64);
    let skip = layout.notify_skip(read16(base + common::QUEUE_NOTIFY_OFF));
    let notify = map(h, layout.notify, skip, NOTIFY)?;
    write16(base + common::QUEUE_ENABLE, 1);
    Some((frame, size, notify))
}

/// What the block device the kernel drives is like, once it started. Only
/// the test image asks.
#[cfg(feature = "qemu-test")]
pub fn drive() -> Option<Drive> {
    QUEUE.lock().as_ref().map(|q| q.drive)
}

/// Reads the sectors from `sector` on into `into`, which takes a whole
/// number of them, a frame at a time.
///
/// # Errors
///
/// When there is no device, the sectors are not all on it, no memory is
/// left for the frame they come through, or the device does not answer in
/// time or answers with an error.
pub fn read(sector: u64, into: &mut [u8]) -> Result<(), Error> {
    let mut guard = QUEUE.lock();
    let queue = guard.as_mut().ok_or(Error::NoDrive)?;
    span(queue, sector, into.len())?;
    let data = memory::device_frame().ok_or(Error::NoMemory)?;
    let mut answer = Ok(());
    for (first, part) in block::parts(sector, into.len()) {
        let chunk = &mut into[part];
        let header = virtio::read_header(first);
        answer = request(queue, &header, false, data, chunk.len());
        if answer.is_err() {
            break;
        }
        // SAFETY: the frame is the kernel's alone, in the direct map, and
        // the device wrote the chunk's length into it before it answered.
        let read =
            unsafe { core::slice::from_raw_parts((HHDM_BASE + data) as *const u8, chunk.len()) };
        chunk.copy_from_slice(read);
    }
    memory::give_back(data);
    answer
}

/// Writes `from`, a whole number of sectors, from `sector` on, a frame at
/// a time.
///
/// # Errors
///
/// As for [`read`], and when the device only reads.
// Only the test image writes yet, until files can be written.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn write(sector: u64, from: &[u8]) -> Result<(), Error> {
    let mut guard = QUEUE.lock();
    let queue = guard.as_mut().ok_or(Error::NoDrive)?;
    if queue.drive.read_only {
        return Err(Error::ReadOnly);
    }
    span(queue, sector, from.len())?;
    let data = memory::device_frame().ok_or(Error::NoMemory)?;
    let mut answer = Ok(());
    for (first, part) in block::parts(sector, from.len()) {
        let chunk = &from[part];
        // SAFETY: the frame is the kernel's alone, in the direct map, and a
        // chunk is no longer than it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                (HHDM_BASE + data) as *mut u8,
                chunk.len(),
            );
        };
        let header = virtio::write_header(first);
        answer = request(queue, &header, true, data, chunk.len());
        if answer.is_err() {
            break;
        }
    }
    memory::give_back(data);
    answer
}

/// Whether the `len` bytes from `sector` on are a whole number of sectors,
/// all on the device.
fn span(queue: &Queue, sector: u64, len: usize) -> Result<(), Error> {
    let len = len as u64;
    let whole = len > 0 && len.is_multiple_of(block::SECTOR);
    let end = sector.checked_add(len / block::SECTOR);
    if whole && end.is_some_and(|end| end <= queue.drive.sectors) {
        Ok(())
    } else {
        Err(Error::OutOfRange)
    }
}

/// Sends the request `header` with the `length` bytes of the frame `data`,
/// for the device to read when `out` and to write otherwise, and waits for
/// the answer.
fn request(
    queue: &mut Queue,
    header: &[u8; 16],
    out: bool,
    data: u64,
    length: usize,
) -> Result<(), Error> {
    let frame = HHDM_BASE + queue.frame;
    let rings = virtio::rings(queue.size);
    let length = u32::try_from(length).map_err(|_| Error::OutOfRange)?;
    let (header_at, status_at) = (queue.frame + HEADER, queue.frame + STATUS);
    let chain = if out {
        virtio::write_chain(header_at, data, length, status_at)
    } else {
        virtio::read_chain(header_at, data, length, status_at)
    };
    let status = (HHDM_BASE + status_at) as *mut u8;
    // SAFETY: the queue's frame is the kernel's and the device's alone, in
    // the direct map; the device reads the descriptors and the request only
    // once the driver's ring says so below, and with one request at a time
    // it is done with the earlier one. Every entry of the driver's ring
    // names descriptor 0, the head of the only chain, as the zeroed frame
    // had it from the start.
    unsafe {
        core::ptr::copy_nonoverlapping(chain.as_ptr().cast::<u8>(), frame as *mut u8, 48);
        core::ptr::copy_nonoverlapping(header.as_ptr(), (HHDM_BASE + header_at) as *mut u8, 16);
        core::ptr::write_volatile(status, 0xff);
        let ring = frame + rings.available as u64;
        fence(Ordering::SeqCst);
        core::ptr::write_volatile((ring + 2) as *mut u16, queue.sent.wrapping_add(1));
    }
    fence(Ordering::SeqCst);
    write16(queue.notify, 0);
    queue.sent = queue.sent.wrapping_add(1);
    let used = frame + rings.used as u64;
    // SAFETY: as above; the device writes its ring's index as it answers.
    let answered =
        wait(|| unsafe { core::ptr::read_volatile((used + 2) as *const u16) } == queue.sent);
    if !answered {
        return Err(Error::TimedOut);
    }
    fence(Ordering::SeqCst);
    // SAFETY: as above; the device wrote the status before its ring.
    match unsafe { core::ptr::read_volatile(status) } {
        block::OK => Ok(()),
        other => Err(Error::Failed(other)),
    }
}

/// Sends a request of the type `kind`, for sector 0 and one sector of data,
/// and answers how the device took it. Only the test image asks, for a
/// type no device knows.
#[cfg(feature = "qemu-test")]
pub fn ask(kind: u32) -> Result<(), Error> {
    let mut guard = QUEUE.lock();
    let queue = guard.as_mut().ok_or(Error::NoDrive)?;
    let data = memory::device_frame().ok_or(Error::NoMemory)?;
    let mut header = virtio::read_header(0);
    header[..4].copy_from_slice(&kind.to_le_bytes());
    let answer = request(queue, &header, false, data, 512);
    memory::give_back(data);
    answer
}

/// The device's status, the features it offers and those agreed on, as
/// the device reads them back, and the size and state of its first queue.
/// Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn agreed() -> Option<(u8, u64, u64, u16, u16)> {
    let guard = QUEUE.lock();
    guard.as_ref()?;
    let features = |select_at: u64, at: u64| {
        let half = |select: u32| {
            write32(COMMON + select_at, select);
            read32(COMMON + at)
        };
        x86::join(half(1), half(0))
    };
    let offered = features(common::DEVICE_FEATURE_SELECT, common::DEVICE_FEATURE);
    let taken = features(common::DRIVER_FEATURE_SELECT, common::DRIVER_FEATURE);
    write16(COMMON + common::QUEUE_SELECT, 0);
    Some((
        read8(COMMON + common::DEVICE_STATUS),
        offered,
        taken,
        read16(COMMON + common::QUEUE_SIZE),
        read16(COMMON + common::QUEUE_ENABLE),
    ))
}
