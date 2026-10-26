//! virtio over PCI, as version 1.1 of the OASIS specification has it, the
//! parts that need no hardware: where a device keeps its configuration
//! structures, which of its features to take, how a split virtqueue lies in
//! memory and what its descriptors hold, and the requests a block device
//! takes. The kernel touches the registers; this crate tells what goes
//! where.

#![cfg_attr(not(test), no_std)]

/// The PCI vendor of every virtio device.
pub const VENDOR: u16 = 0x1af4;

/// The PCI device id of a block device: 0x1040 plus its virtio id, 2, for
/// a device of version 1, and 0x1001 for one that also speaks the older
/// interface.
pub const BLOCK: u16 = 0x1042;
pub const BLOCK_TRANSITIONAL: u16 = 0x1001;

/// The PCI capability id virtio's configuration structures are listed
/// under.
pub const CAPABILITY: u8 = 0x09;

/// Kinds of configuration structure, as `cfg_type` names them.
pub mod kind {
    pub const COMMON: u8 = 1;
    pub const NOTIFY: u8 = 2;
    pub const ISR: u8 = 3;
    pub const DEVICE: u8 = 4;
}

/// Where a configuration structure lies: in which base address register,
/// how far into it and how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub bar: u8,
    pub offset: u32,
    pub length: u32,
}

impl Region {
    /// Where byte `skip` of the region lies, for a base address register
    /// that starts at `bar`.
    #[must_use]
    pub fn at(&self, bar: u64, skip: u64) -> u64 {
        bar + u64::from(self.offset) + skip
    }
}

/// The page that holds the register at physical `at`, and where the
/// register lies once that page is mapped at `virt`.
#[must_use]
pub fn window(at: u64, virt: u64) -> (u64, u64) {
    (at & !0xfff, virt + (at & 0xfff))
}

/// Where the low and the high half of the 64-bit register at `at` lie, as
/// it is read and written: two 32-bit registers, the low one first.
#[must_use]
pub fn halves(at: u64) -> [u64; 2] {
    [at, at + 4]
}

/// Where a device keeps its configuration structures, and how far apart
/// the notification addresses of its queues are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub common: Region,
    pub notify: Region,
    pub notify_multiplier: u32,
    pub isr: Region,
    pub device: Region,
}

impl Layout {
    /// How far into the notification region a queue is rung whose
    /// notification offset is `off`.
    #[must_use]
    pub fn notify_skip(&self, off: u16) -> u64 {
        u64::from(off) * u64::from(self.notify_multiplier)
    }
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The layout configuration space `config` gives, from the first capability
/// of each kind it lists; none when one is missing or cut short.
#[must_use]
pub fn layout(config: &[u8]) -> Option<Layout> {
    let (mut common, mut notify, mut isr, mut device) = (None, None, None, None);
    let mut multiplier = None;
    for (_, at) in pci::capabilities(config).filter(|&(id, _)| id == CAPABILITY) {
        let region = Region {
            bar: *config.get(at + 4)?,
            offset: word(config, at + 8)?,
            length: word(config, at + 12)?,
        };
        match config.get(at + 3) {
            Some(&kind::COMMON) => _ = common.get_or_insert(region),
            Some(&kind::NOTIFY) if notify.is_none() => {
                notify = Some(region);
                multiplier = Some(word(config, at + 16)?);
            }
            Some(&kind::ISR) => _ = isr.get_or_insert(region),
            Some(&kind::DEVICE) => _ = device.get_or_insert(region),
            _ => {}
        }
    }
    Some(Layout {
        common: common?,
        notify: notify?,
        notify_multiplier: multiplier?,
        isr: isr?,
        device: device?,
    })
}

/// Where the registers of the common configuration structure are, from its
/// start, with their widths in bytes.
pub mod common {
    pub const DEVICE_FEATURE_SELECT: u64 = 0x00;
    pub const DEVICE_FEATURE: u64 = 0x04;
    pub const DRIVER_FEATURE_SELECT: u64 = 0x08;
    pub const DRIVER_FEATURE: u64 = 0x0c;
    /// 16 bits.
    pub const NUM_QUEUES: u64 = 0x12;
    /// 8 bits.
    pub const DEVICE_STATUS: u64 = 0x14;
    /// 16 bits each.
    pub const QUEUE_SELECT: u64 = 0x16;
    pub const QUEUE_SIZE: u64 = 0x18;
    pub const QUEUE_ENABLE: u64 = 0x1c;
    pub const QUEUE_NOTIFY_OFF: u64 = 0x1e;
    /// 64 bits each: where the descriptors, the driver's ring and the
    /// device's ring of the selected queue are.
    pub const QUEUE_DESC: u64 = 0x20;
    pub const QUEUE_DRIVER: u64 = 0x28;
    pub const QUEUE_DEVICE: u64 = 0x30;
}

/// The bits of the device status.
pub mod status {
    pub const ACKNOWLEDGE: u8 = 1;
    pub const DRIVER: u8 = 2;
    pub const DRIVER_OK: u8 = 4;
    pub const FEATURES_OK: u8 = 8;
    pub const NEEDS_RESET: u8 = 0x40;
    pub const FAILED: u8 = 0x80;
}

/// Feature bits.
pub mod features {
    /// The device speaks version 1, the interface this crate knows.
    pub const VERSION_1: u64 = 1 << 32;
    /// A block device that only reads.
    pub const BLOCK_READ_ONLY: u64 = 1 << 5;
}

/// The features to take of those a block device offers: version 1, which
/// it has to offer, and that it only reads, which it says that way.
#[must_use]
pub fn accept(offered: u64) -> Option<u64> {
    (offered & features::VERSION_1 != 0)
        .then_some(offered & (features::VERSION_1 | features::BLOCK_READ_ONLY))
}

/// Whether the features taken say the device only reads.
#[must_use]
pub fn read_only(taken: u64) -> bool {
    taken & features::BLOCK_READ_ONLY != 0
}

/// Whether the device still says FEATURES_OK once the driver set it with
/// the features it took. A device that clears it again cannot work with
/// those and is not used.
#[must_use]
pub fn features_kept(device_status: u8) -> bool {
    device_status & status::FEATURES_OK != 0
}

/// How many entries the queue Meridian sets up has, at most: a read needs
/// three descriptors, and there is one read at a time.
pub const QUEUE_SIZE: u16 = 8;

/// How many entries the queue gets where the device has room for `room`:
/// no more than [`QUEUE_SIZE`], and none when fewer than the three one
/// request takes.
#[must_use]
pub fn queue_size(room: u16) -> Option<u16> {
    let size = room.min(QUEUE_SIZE);
    (size >= 3).then_some(size)
}

/// Where the parts of a split virtqueue lie in its memory: the descriptor
/// table from the start, the driver's ring right after it, and the
/// device's ring from the next multiple of 4 on, up to `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rings {
    pub available: usize,
    pub used: usize,
    pub end: usize,
}

/// The rings of a queue of `size` entries.
#[must_use]
pub fn rings(size: u16) -> Rings {
    let n = usize::from(size);
    let available = 16 * n;
    let used = (available + 6 + 2 * n).next_multiple_of(4);
    Rings {
        available,
        used,
        end: used + 6 + 8 * n,
    }
}

/// Descriptor flags: another descriptor follows, and the device writes
/// the buffer instead of reading it.
pub const NEXT: u16 = 1;
pub const WRITE: u16 = 2;

/// A descriptor of the `length` bytes at `address`, followed by descriptor
/// `next` when `flags` has [`NEXT`].
#[must_use]
pub fn descriptor(address: u64, length: u32, flags: u16, next: u16) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&address.to_le_bytes());
    d[8..12].copy_from_slice(&length.to_le_bytes());
    d[12..14].copy_from_slice(&flags.to_le_bytes());
    d[14..].copy_from_slice(&next.to_le_bytes());
    d
}

/// Block requests and their answers.
pub mod block {
    /// A read and a write.
    pub const IN: u32 = 0;
    pub const OUT: u32 = 1;
    /// What the status byte says.
    pub const OK: u8 = 0;
    pub const IO_ERROR: u8 = 1;
    pub const UNSUPPORTED: u8 = 2;
    /// The size of the sectors requests count in, whatever the disk's.
    pub const SECTOR: u64 = 512;

    /// The most bytes one request moves: a frame's worth.
    pub const PART: usize = 4096;

    /// The parts of [`PART`] bytes or less that `len` bytes from `sector`
    /// on go in: the sector each starts at, and where in the bytes it lies.
    pub fn parts(sector: u64, len: usize) -> impl Iterator<Item = (u64, core::ops::Range<usize>)> {
        (0..len).step_by(PART).map(move |start| {
            let first = sector + start as u64 / SECTOR;
            (first, start..len.min(start + PART))
        })
    }
}

/// The header of a request to read from `sector` on.
#[must_use]
pub fn read_header(sector: u64) -> [u8; 16] {
    header(block::IN, sector)
}

/// The header of a request to write from `sector` on.
#[must_use]
pub fn write_header(sector: u64) -> [u8; 16] {
    header(block::OUT, sector)
}

fn header(kind: u32, sector: u64) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(&kind.to_le_bytes());
    h[8..].copy_from_slice(&sector.to_le_bytes());
    h
}

/// The three descriptors of a read, as entries 0, 1 and 2 of the table:
/// the header at `header`, which the device reads, the `length` bytes at
/// `data` and then the status byte at `status`, which it writes.
#[must_use]
pub fn read_chain(header: u64, data: u64, length: u32, status: u64) -> [[u8; 16]; 3] {
    [
        descriptor(header, 16, NEXT, 1),
        descriptor(data, length, NEXT | WRITE, 2),
        descriptor(status, 1, WRITE, 0),
    ]
}

/// The three descriptors of a write, as [`read_chain`] lays out a read:
/// the device reads the data instead of writing it.
#[must_use]
pub fn write_chain(header: u64, data: u64, length: u32, status: u64) -> [[u8; 16]; 3] {
    [
        descriptor(header, 16, NEXT, 1),
        descriptor(data, length, NEXT, 2),
        descriptor(status, 1, WRITE, 0),
    ]
}

/// The most data descriptors one request of [`chain`] has: the queue's
/// entries but the header's and the status byte's.
pub const PIECES: usize = QUEUE_SIZE as usize - 2;

/// The descriptor table of a request whose data lies in `pieces`, each
/// at its physical address with its length: the header at `header`, the
/// pieces in order, which the device writes when `device_writes` and reads
/// otherwise, and the status byte at `status`, and how many of the table's
/// entries the request takes. None for no pieces, or more than [`PIECES`].
#[must_use]
pub fn chain(
    header: u64,
    pieces: &[(u64, u32)],
    device_writes: bool,
    status: u64,
) -> Option<([[u8; 16]; QUEUE_SIZE as usize], usize)> {
    if pieces.is_empty() || pieces.len() > PIECES {
        return None;
    }
    let flags = if device_writes { NEXT | WRITE } else { NEXT };
    let mut table = [[0u8; 16]; QUEUE_SIZE as usize];
    table[0] = descriptor(header, 16, NEXT, 1);
    for (n, &(address, length)) in (1u16..).zip(pieces) {
        table[usize::from(n)] = descriptor(address, length, flags, n + 1);
    }
    let last = pieces.len() + 1;
    table[last] = descriptor(status, 1, WRITE, 0);
    Some((table, last + 1))
}

/// How many 512-byte sectors a block device holds, from the start of its
/// device configuration.
#[must_use]
pub fn capacity(device: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(device.get(..8)?.try_into().ok()?))
}

/// The entry of the device's ring a request with index `index` answered
/// in: the head of its descriptors and how many bytes it wrote, in a ring
/// of `size` entries.
#[must_use]
pub fn used_entry(ring: &[u8], index: u16, size: u16) -> Option<(u32, u32)> {
    let at = 4 + 8 * usize::from(index % size.max(1));
    Some((word(ring, at)?, word(ring, at + 4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_lie_where_their_bar_and_region_put_them() {
        let region = Region {
            bar: 4,
            offset: 0x3000,
            length: 0x1000,
        };
        assert_eq!(region.at(0xfe00_0000, 0), 0xfe00_3000);
        assert_eq!(region.at(0xfe00_0000, 0x18), 0xfe00_3018, "and further in");
        assert_eq!(
            window(0xfe00_3018, 0xffff_0000_0000_2000),
            (0xfe00_3000, 0xffff_0000_0000_2018)
        );
        assert_eq!(
            window(0xfe00_4000, 0x7000),
            (0xfe00_4000, 0x7000),
            "a register that starts its page"
        );
        assert_eq!(halves(0x1000), [0x1000, 0x1004]);
    }

    #[test]
    fn queues_are_rung_as_far_apart_as_the_multiplier_says() {
        let region = Region {
            bar: 4,
            offset: 0,
            length: 0,
        };
        let layout = Layout {
            common: region,
            notify: region,
            notify_multiplier: 4,
            isr: region,
            device: region,
        };
        assert_eq!([0, 1, 3].map(|off| layout.notify_skip(off)), [0, 4, 12]);
        let wide = Layout {
            notify_multiplier: 0x1000,
            ..layout
        };
        assert_eq!(wide.notify_skip(u16::MAX), 0x0fff_f000, "a page apart");
    }

    #[test]
    fn a_queue_takes_what_room_there_is_up_to_its_own() {
        assert_eq!(
            [0, 2, 3, 7, 8, 9, 256].map(queue_size),
            [None, None, Some(3), Some(7), Some(8), Some(8), Some(8)]
        );
        assert!(read_only(features::VERSION_1 | features::BLOCK_READ_ONLY));
        assert!(!read_only(features::VERSION_1));
    }

    #[test]
    fn requests_go_a_frame_at_a_time() {
        let parts = |sector, len| block::parts(sector, len).collect::<Vec<_>>();
        assert_eq!(parts(10, 0), []);
        assert_eq!(parts(10, 512), [(10, 0..512)]);
        assert_eq!(parts(10, 4096), [(10, 0..4096)]);
        assert_eq!(
            parts(10, 2 * 4096 + 512),
            [(10, 0..4096), (18, 4096..8192), (26, 8192..8704)],
            "each part eight sectors after the one before"
        );
    }

    /// A capability of `kind` at `at` in `config`, pointing at the next one.
    fn capability(config: &mut [u8], at: usize, next: u8, kind: u8, region: Region) {
        config[at] = CAPABILITY;
        config[at + 1] = next;
        config[at + 2] = 16;
        config[at + 3] = kind;
        config[at + 4] = region.bar;
        config[at + 8..at + 12].copy_from_slice(&region.offset.to_le_bytes());
        config[at + 12..at + 16].copy_from_slice(&region.length.to_le_bytes());
    }

    fn region(bar: u8, offset: u32, length: u32) -> Region {
        Region {
            bar,
            offset,
            length,
        }
    }

    /// The capabilities QEMU 8.2's virtio-blk-pci lists, as the kernel
    /// read them: MSI-X's at 0x98 first, then one for reaching the
    /// structures through configuration space, which Meridian does not use,
    /// the notification structure with its multiplier, the device's, the
    /// ISR's and the common one, every structure in the fourth base address
    /// register.
    fn qemu_config() -> [u8; 256] {
        let mut config = [0u8; 256];
        config[6] = 1 << 4;
        config[0x34] = 0x98;
        config[0x98] = 0x11;
        config[0x99] = 0x84;
        capability(&mut config, 0x84, 0x70, 5, region(0, 0, 0));
        config[0x86] = 20;
        capability(
            &mut config,
            0x70,
            0x60,
            kind::NOTIFY,
            region(4, 0x3000, 0x1000),
        );
        config[0x72] = 20;
        config[0x80..0x84].copy_from_slice(&4u32.to_le_bytes());
        capability(
            &mut config,
            0x60,
            0x50,
            kind::DEVICE,
            region(4, 0x2000, 0x1000),
        );
        capability(
            &mut config,
            0x50,
            0x40,
            kind::ISR,
            region(4, 0x1000, 0x1000),
        );
        capability(&mut config, 0x40, 0, kind::COMMON, region(4, 0, 0x1000));
        config
    }

    #[test]
    fn the_layout_comes_from_the_capabilities() {
        assert_eq!(
            layout(&qemu_config()),
            Some(Layout {
                common: region(4, 0, 0x1000),
                notify: region(4, 0x3000, 0x1000),
                notify_multiplier: 4,
                isr: region(4, 0x1000, 0x1000),
                device: region(4, 0x2000, 0x1000),
            })
        );
    }

    #[test]
    fn a_layout_needs_every_structure() {
        let mut config = qemu_config();
        config[0x40 + 3] = 9;
        assert_eq!(layout(&config), None, "no common structure");
        let mut other = qemu_config();
        other[0x50] = 0x05;
        assert_eq!(layout(&other), None, "the ISR's capability is not virtio's");
        assert_eq!(layout(&qemu_config()[..0x48]), None, "cut short");
        assert_eq!(layout(&[0; 256]), None);
    }

    #[test]
    fn the_first_of_each_kind_counts() {
        let mut config = qemu_config();
        capability(&mut config, 0xc0, 0, kind::COMMON, region(2, 0x500, 0x40));
        config[0x41] = 0xc0;
        let first = layout(&config).unwrap();
        assert_eq!(first.common, region(4, 0, 0x1000));
        capability(&mut config, 0xc0, 0, kind::NOTIFY, region(2, 0x600, 0x40));
        config[0xd0..0xd4].copy_from_slice(&8u32.to_le_bytes());
        let notify = layout(&config).unwrap();
        assert_eq!(
            (notify.notify, notify.notify_multiplier),
            (region(4, 0x3000, 0x1000), 4),
            "and its multiplier with it"
        );
    }

    #[test]
    fn only_version_1_is_spoken() {
        let offered = features::VERSION_1 | features::BLOCK_READ_ONLY | 1 << 9 | 1 << 33;
        assert_eq!(
            accept(offered),
            Some(features::VERSION_1 | features::BLOCK_READ_ONLY)
        );
        assert_eq!(
            accept(features::VERSION_1 | 1 << 6),
            Some(features::VERSION_1)
        );
        assert_eq!(accept(features::BLOCK_READ_ONLY), None, "no version 1");
        assert_eq!(accept(0), None);
        assert_eq!(
            (features::VERSION_1, features::BLOCK_READ_ONLY),
            (1 << 32, 32)
        );
    }

    #[test]
    fn the_rings_follow_the_descriptors() {
        assert_eq!(
            rings(8),
            Rings {
                available: 128,
                used: 152,
                end: 222
            }
        );
        assert_eq!(rings(1).used, 24, "22 bytes in, rounded up to 4");
        assert_eq!(rings(256).end, 4096 + 518 + 2 + 6 + 2048);
        assert!(
            rings(QUEUE_SIZE).end <= 2048,
            "with room for requests in the same page"
        );
    }

    #[test]
    fn a_descriptor_holds_its_buffer_and_what_follows() {
        let d = descriptor(0x1234_5678_9abc, 512, NEXT | WRITE, 2);
        assert_eq!(&d[..8], 0x1234_5678_9abc_u64.to_le_bytes());
        assert_eq!(&d[8..12], [0, 2, 0, 0]);
        assert_eq!(&d[12..], [3, 0, 2, 0]);
        assert_eq!((NEXT, WRITE), (1, 2));
    }

    #[test]
    fn a_read_is_a_header_the_data_and_a_status() {
        assert_eq!(
            read_header(0x0102_0304_0506_0708),
            [0, 0, 0, 0, 0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]
        );
        let chain = read_chain(0x1000, 0x2000, 4096, 0x1010);
        assert_eq!(chain[0], descriptor(0x1000, 16, NEXT, 1));
        assert_eq!(chain[1], descriptor(0x2000, 4096, NEXT | WRITE, 2));
        assert_eq!(chain[2], descriptor(0x1010, 1, WRITE, 0));
        assert_eq!(
            (block::IN, block::OK, block::IO_ERROR, block::UNSUPPORTED),
            (0, 0, 1, 2)
        );
    }

    #[test]
    fn the_capacity_and_the_used_ring_are_read() {
        let mut device = [0u8; 60];
        device[..8].copy_from_slice(&135_168u64.to_le_bytes());
        assert_eq!(capacity(&device), Some(135_168));
        assert_eq!(capacity(&device[..7]), None);
        let mut used = [0u8; 6 + 8 * 8];
        used[4 + 8 * 3..4 + 8 * 3 + 4].copy_from_slice(&0u32.to_le_bytes());
        used[4 + 8 * 3 + 4..4 + 8 * 4].copy_from_slice(&4097u32.to_le_bytes());
        assert_eq!(used_entry(&used, 3, 8), Some((0, 4097)));
        assert_eq!(used_entry(&used, 11, 8), Some((0, 4097)), "the index wraps");
        assert_eq!(used_entry(&used[..30], 3, 8), None);
        assert_eq!(used_entry(&used, 0, 0), Some((0, 0)), "no ring of 0");
    }

    /// A descriptor read back as the specification lays it out: address,
    /// length, flags, next.
    fn decoded(d: &[u8]) -> (u64, u32, u16, u16) {
        (
            u64::from_le_bytes(d[..8].try_into().unwrap()),
            u32::from_le_bytes(d[8..12].try_into().unwrap()),
            u16::from_le_bytes([d[12], d[13]]),
            u16::from_le_bytes([d[14], d[15]]),
        )
    }

    #[test]
    fn a_request_takes_as_many_pieces_as_the_queue_leaves_room_for() {
        let pieces: Vec<(u64, u32)> = (1..=6).map(|n| (n << 12, 4096)).collect();
        let (table, used) = chain(0x9000, &pieces, true, 0x9010).unwrap();
        assert_eq!(used, 8, "the header, six pieces and the status");
        assert_eq!(table[0], descriptor(0x9000, 16, NEXT, 1));
        for n in 1..=6u16 {
            assert_eq!(
                table[usize::from(n)],
                descriptor(u64::from(n) << 12, 4096, NEXT | WRITE, n + 1),
                "piece {n}"
            );
        }
        assert_eq!(table[7], descriptor(0x9010, 1, WRITE, 0));
        let seven: Vec<(u64, u32)> = (1..=7).map(|n| (n << 12, 4096)).collect();
        assert_eq!(chain(0x9000, &seven, true, 0x9010), None, "one too many");
        assert_eq!(chain(0x9000, &[], true, 0x9010), None, "no data");
    }

    #[test]
    fn one_piece_is_the_chain_of_a_read_or_a_write() {
        let (read, used) = chain(0x1000, &[(0x2000, 1024)], true, 0x1010).unwrap();
        assert_eq!(
            (&read[..3], used),
            (&read_chain(0x1000, 0x2000, 1024, 0x1010)[..], 3)
        );
        assert!(read[3..].iter().all(|d| *d == [0; 16]), "the rest unused");
        let (write, _) = chain(0x1000, &[(0x2000, 1024)], false, 0x1010).unwrap();
        assert_eq!(&write[..3], &write_chain(0x1000, 0x2000, 1024, 0x1010)[..]);
        let (pair, used) = chain(0x1000, &[(0x2000, 4096), (0x7000, 512)], false, 0x1010).unwrap();
        assert_eq!(used, 4);
        assert_eq!(pair[1], descriptor(0x2000, 4096, NEXT, 2));
        assert_eq!(
            pair[2],
            descriptor(0x7000, 512, NEXT, 3),
            "the device reads what it writes out"
        );
        assert_eq!(pair[3], descriptor(0x1010, 1, WRITE, 0));
    }

    #[test]
    fn a_read_chain_is_followed_from_its_head_to_its_end() {
        let mut table = [0u8; 16 * 8];
        for (i, d) in read_chain(0x5000, 0x6000, 512, 0x5010).iter().enumerate() {
            table[16 * i..16 * i + 16].copy_from_slice(d);
        }
        let mut at = 0;
        let mut seen = Vec::new();
        loop {
            let (address, length, flags, next) = decoded(&table[16 * at..16 * at + 16]);
            seen.push((address, length, flags & WRITE != 0));
            if flags & NEXT == 0 {
                break;
            }
            at = usize::from(next);
        }
        assert_eq!(
            seen,
            [(0x5000, 16, false), (0x6000, 512, true), (0x5010, 1, true)],
            "the device reads the header and writes the data and the status"
        );
    }

    #[test]
    fn a_write_hands_the_device_its_data_to_read() {
        assert_eq!(write_header(9)[..4], 1u32.to_le_bytes());
        assert_eq!(write_header(9)[4..], read_header(9)[4..]);
        let chain = write_chain(0x1000, 0x2000, 1024, 0x1010);
        let flags: Vec<u16> = chain.iter().map(|d| decoded(d).2).collect();
        assert_eq!(
            flags,
            [NEXT, NEXT, WRITE],
            "only the status for the device to write"
        );
        assert_eq!(decoded(&chain[1]), (0x2000, 1024, NEXT, 2));
        assert_eq!(chain[0], read_chain(0x1000, 0x2000, 1024, 0x1010)[0]);
        assert_eq!(block::OUT, 1);
    }

    #[test]
    fn the_common_structure_is_laid_out_as_the_specification_has_it() {
        let registers = [
            (common::DEVICE_FEATURE_SELECT, 0x00),
            (common::DEVICE_FEATURE, 0x04),
            (common::DRIVER_FEATURE_SELECT, 0x08),
            (common::DRIVER_FEATURE, 0x0c),
            (common::NUM_QUEUES, 0x12),
            (common::DEVICE_STATUS, 0x14),
            (common::QUEUE_SELECT, 0x16),
            (common::QUEUE_SIZE, 0x18),
            (common::QUEUE_ENABLE, 0x1c),
            (common::QUEUE_NOTIFY_OFF, 0x1e),
            (common::QUEUE_DESC, 0x20),
            (common::QUEUE_DRIVER, 0x28),
            (common::QUEUE_DEVICE, 0x30),
        ];
        assert!(registers.iter().all(|&(at, want)| at == want));
        assert_eq!(
            [
                status::ACKNOWLEDGE,
                status::DRIVER,
                status::DRIVER_OK,
                status::FEATURES_OK
            ],
            [1, 2, 4, 8]
        );
        assert_eq!((status::NEEDS_RESET, status::FAILED), (0x40, 0x80));
    }

    #[test]
    fn a_device_that_clears_features_ok_again_has_not_kept_the_features() {
        let set = status::ACKNOWLEDGE | status::DRIVER;
        assert!(features_kept(set | status::FEATURES_OK));
        assert!(features_kept(0xff));
        assert!(!features_kept(set), "cleared again");
        assert!(!features_kept(
            set | status::DRIVER_OK | status::NEEDS_RESET | status::FAILED
        ));
        assert!(!features_kept(0));
    }

    #[test]
    fn rings_fit_queues_of_every_size_the_driver_may_pick() {
        for size in [1u16, 2, 4, 8, 16, 32, 64] {
            let r = rings(size);
            let n = usize::from(size);
            assert_eq!(r.available, 16 * n, "{size}");
            assert_eq!(r.used % 4, 0, "{size}");
            assert!(r.used >= r.available + 6 + 2 * n && r.used < r.available + 10 + 2 * n);
            assert_eq!(r.end, r.used + 6 + 8 * n, "{size}");
        }
    }

    #[test]
    fn a_capability_cut_short_gives_no_layout() {
        let mut config = qemu_config();
        config[0x34] = 0x70;
        assert!(
            layout(&config).is_some(),
            "the list may start at the notification one"
        );
        assert_eq!(layout(&config[..0x7c]), None, "its region cut");
        assert_eq!(layout(&config[..0x83]), None, "its multiplier cut");
        let mut no_list = config;
        no_list[6] = 0;
        assert_eq!(layout(&no_list), None, "no list at all");
    }

    #[test]
    fn devices_are_known_by_their_ids() {
        assert_eq!(
            (VENDOR, BLOCK, BLOCK_TRANSITIONAL, CAPABILITY),
            (0x1af4, 0x1042, 0x1001, 9)
        );
        assert_eq!(
            (kind::COMMON, kind::NOTIFY, kind::ISR, kind::DEVICE),
            (1, 2, 3, 4)
        );
        assert_eq!(common::QUEUE_DEVICE, 0x30);
        assert_eq!(status::FEATURES_OK | status::DRIVER_OK, 12);
    }
}
