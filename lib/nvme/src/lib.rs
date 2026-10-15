//! NVM Express, the parts that need no hardware (base specification
//! 2.0): where a controller's registers are and what their bits mean, the
//! 64-byte commands of a submission queue, the 16-byte entries of a
//! completion queue, and what IDENTIFY says about a controller and its
//! namespaces. The kernel reads and writes the registers and the queues;
//! this crate says what goes there and what came back.

#![cfg_attr(not(test), no_std)]

/// The controller's registers, by their offset from its base.
pub mod register {
    pub const CAPABILITIES: usize = 0x00;
    pub const VERSION: usize = 0x08;
    pub const CONFIGURATION: usize = 0x14;
    pub const STATUS: usize = 0x1c;
    pub const ADMIN_QUEUES: usize = 0x24;
    pub const ADMIN_SUBMISSION: usize = 0x28;
    pub const ADMIN_COMPLETION: usize = 0x30;
    /// Where the doorbells start.
    pub const DOORBELLS: usize = 0x1000;
    /// In `STATUS`: the controller is ready, or failed for good.
    pub const READY: u32 = 1;
    pub const FATAL: u32 = 1 << 1;
}

/// What the capabilities register says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The most entries a queue may have.
    pub queue_entries: u32,
    /// How many bytes lie between one doorbell and the next.
    pub doorbell_stride: usize,
    /// How long the controller may take to become ready or not, in
    /// milliseconds.
    pub timeout_ms: u32,
    /// Whether it speaks the NVM command set.
    pub nvm: bool,
    /// The smallest page it takes, in bytes.
    pub page_min: u64,
}

impl Capabilities {
    #[must_use]
    pub fn read(value: u64) -> Capabilities {
        Capabilities {
            queue_entries: low(value & 0xffff) + 1,
            doorbell_stride: 4 << (value >> 32 & 0xf),
            timeout_ms: low(value >> 24 & 0xff) * 500,
            nvm: value >> 37 & 1 == 1,
            page_min: 1 << (12 + (value >> 48 & 0xf)),
        }
    }

    /// Whether Meridian can drive the controller: it speaks the NVM command
    /// set, takes pages of 4 KiB and queues of `entries`, and the doorbells
    /// of the admin queues and one pair more lie in the first `room` bytes
    /// of its registers.
    #[must_use]
    pub fn usable(&self, entries: u16, room: usize) -> bool {
        self.nvm
            && self.page_min == 4096
            && u32::from(entries) <= self.queue_entries
            && self.doorbell(1, true) + 4 <= room
    }

    /// Where the doorbell of submission queue `queue` lies; the one of its
    /// completion queue is the next.
    #[must_use]
    pub fn doorbell(&self, queue: u16, completion: bool) -> usize {
        register::DOORBELLS
            + (2 * usize::from(queue) + usize::from(completion)) * self.doorbell_stride
    }
}

/// How far turning a controller on got, by its status register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not ready, and not failed either.
    Off,
    Ready,
    /// Failed for good, whether it says it is ready or not.
    Failed,
}

impl State {
    #[must_use]
    pub fn read(status: u32) -> State {
        if status & register::FATAL != 0 {
            State::Failed
        } else if status & register::READY != 0 {
            State::Ready
        } else {
            State::Off
        }
    }
}

/// The configuration that turns a controller on for the NVM command set,
/// 4 KiB pages, and queue entries of 64 bytes to submit and 16 to complete.
pub const ENABLED: u32 = 1 | 6 << 16 | 4 << 20;

/// The attributes of the admin queues: `entries` in each.
#[must_use]
pub fn admin_queues(entries: u16) -> u32 {
    let size = u32::from(entries.max(2) - 1);
    size | size << 16
}

/// A command, as it lies in a submission queue.
pub type Command = [u8; 64];

/// The opcodes Meridian sends.
pub mod opcode {
    pub const CREATE_SUBMISSION_QUEUE: u8 = 0x01;
    pub const CREATE_COMPLETION_QUEUE: u8 = 0x05;
    pub const IDENTIFY: u8 = 0x06;
    pub const READ: u8 = 0x02;
}

/// The low 32 bits of `value`.
fn low(value: u64) -> u32 {
    u32::try_from(value & 0xffff_ffff).unwrap_or(0)
}

fn command(opcode: u8, id: u16, namespace: u32, data: u64, words: [u32; 6]) -> Command {
    let mut c = [0u8; 64];
    c[0] = opcode;
    c[2..4].copy_from_slice(&id.to_le_bytes());
    c[4..8].copy_from_slice(&namespace.to_le_bytes());
    c[24..32].copy_from_slice(&data.to_le_bytes());
    for (i, word) in words.iter().enumerate() {
        c[40 + 4 * i..44 + 4 * i].copy_from_slice(&word.to_le_bytes());
    }
    c
}

/// IDENTIFY of the controller (`namespace` 0) or of namespace `namespace`,
/// whose 4 KiB of answer go to the page at physical `page`.
#[must_use]
pub fn identify(id: u16, namespace: u32, page: u64) -> Command {
    let what = u32::from(namespace == 0);
    command(opcode::IDENTIFY, id, namespace, page, [what, 0, 0, 0, 0, 0])
}

/// Creates completion queue `queue` of `entries` in the page at `page`,
/// without interrupts.
#[must_use]
pub fn create_completion_queue(id: u16, queue: u16, entries: u16, page: u64) -> Command {
    let size = u32::from(entries.max(2) - 1) << 16 | u32::from(queue);
    command(
        opcode::CREATE_COMPLETION_QUEUE,
        id,
        0,
        page,
        [size, 1, 0, 0, 0, 0],
    )
}

/// Creates submission queue `queue` of `entries` in the page at `page`,
/// whose commands complete in completion queue `completion`.
#[must_use]
pub fn create_submission_queue(
    id: u16,
    queue: u16,
    entries: u16,
    page: u64,
    completion: u16,
) -> Command {
    let size = u32::from(entries.max(2) - 1) << 16 | u32::from(queue);
    command(
        opcode::CREATE_SUBMISSION_QUEUE,
        id,
        0,
        page,
        [size, u32::from(completion) << 16 | 1, 0, 0, 0, 0],
    )
}

/// Reads `blocks` blocks from `lba` on of namespace `namespace` into the
/// page at `page`; none for no blocks or more than a 16-bit count holds.
#[must_use]
pub fn read(id: u16, namespace: u32, lba: u64, blocks: u32, page: u64) -> Option<Command> {
    let count = blocks.checked_sub(1).filter(|&c| c <= 0xffff)?;
    Some(command(
        opcode::READ,
        id,
        namespace,
        page,
        [low(lba), low(lba >> 32), count, 0, 0, 0],
    ))
}

/// A completion queue entry, as the controller wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// How far the controller got in the submission queue.
    pub head: u16,
    /// The id of the command it answers.
    pub id: u16,
    /// Which of the two passes through the queue it was written in.
    pub phase: bool,
    /// Its status: 0 when the command went well.
    pub status: u16,
}

impl Completion {
    #[must_use]
    pub fn read(entry: &[u8; 16]) -> Completion {
        let word = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);
        Completion {
            head: u16::from_le_bytes([entry[8], entry[9]]),
            id: u16::try_from(word & 0xffff).unwrap_or(0),
            phase: word >> 16 & 1 == 1,
            status: u16::try_from(word >> 17).unwrap_or(0),
        }
    }
}

/// What a controller says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controller {
    pub vendor: u16,
    serial: [u8; 20],
    model: [u8; 40],
    /// The most namespaces it may have.
    pub namespaces: u32,
}

/// The bytes of a padded text field, without the padding.
fn trimmed(field: &[u8]) -> &str {
    core::str::from_utf8(field)
        .unwrap_or("")
        .trim_end_matches([' ', '\0'])
}

impl Controller {
    #[must_use]
    pub fn read(data: &[u8; 4096]) -> Controller {
        let mut serial = [0u8; 20];
        serial.copy_from_slice(&data[4..24]);
        let mut model = [0u8; 40];
        model.copy_from_slice(&data[24..64]);
        Controller {
            vendor: u16::from_le_bytes([data[0], data[1]]),
            serial,
            model,
            namespaces: u32::from_le_bytes([data[516], data[517], data[518], data[519]]),
        }
    }

    #[must_use]
    pub fn model(&self) -> &str {
        trimmed(&self.model)
    }

    #[must_use]
    pub fn serial(&self) -> &str {
        trimmed(&self.serial)
    }
}

/// What a namespace says about itself: how many blocks it has and how big
/// each is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace {
    pub blocks: u64,
    pub block_size: u32,
}

impl Namespace {
    /// The namespace in IDENTIFY's answer, or none for one that is not
    /// there: no blocks, or blocks of a size no format names.
    #[must_use]
    pub fn read(data: &[u8; 4096]) -> Option<Namespace> {
        let blocks = u64::from_le_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
        ]);
        let format = usize::from(data[26] & 0xf);
        let at = 128 + 4 * format;
        let shift = u32::from(data[at + 2]);
        if blocks == 0 || !(9..=16).contains(&shift) {
            return None;
        }
        Some(Namespace {
            blocks,
            block_size: 1 << shift,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(c: &Command, at: usize) -> u32 {
        u32::from_le_bytes(c[at..at + 4].try_into().unwrap())
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The capabilities QEMU 9's NVM Express controller reports.
    const QEMU_CAPABILITIES: u64 = 0x0040_1820_0f01_07ff;

    /// What QEMU's controller, started with serial=meridian, answers to
    /// IDENTIFY: every byte that is not 0 among the first 520.
    fn qemu_controller() -> [u8; 4096] {
        let mut data = [0u8; 4096];
        let parts = [
            (
                0,
                concat!(
                    "361bf41a6d6572696469616e20202020202020202020202051454d55204e564d65204374726c",
                    "2020202020202020202020202020202020202020202020202020382e322e3220202006005452",
                    "000700000004010000000000000000000001000000800000000000000000000000000001",
                ),
            ),
            (256, "2a010303030700000000570175010000"),
            (512, "6644000000010000"),
        ];
        for (at, bytes) in parts {
            let bytes = hex(bytes);
            data[at..at + bytes.len()].copy_from_slice(&bytes);
        }
        data
    }

    /// What it answers about namespace 1, the 66 MiB test image.
    fn qemu_namespace() -> [u8; 4096] {
        let mut data = [0u8; 4096];
        let head = hex("001002000000000000100200000000000010020000000000140700031f000000");
        let formats = hex("0000090008000900100009004000090000000c0008000c0010000c0040000c00");
        data[..32].copy_from_slice(&head);
        data[128..160].copy_from_slice(&formats);
        data
    }

    #[test]
    fn qemus_controller_reads_as_it_is() {
        let caps = Capabilities::read(QEMU_CAPABILITIES);
        assert_eq!(
            (caps.queue_entries, caps.doorbell_stride, caps.timeout_ms),
            (2048, 4, 7500)
        );
        assert!(caps.nvm);
        assert_eq!(
            caps.page_min, 4096,
            "pages of 4 KiB are allowed, the ones Meridian uses"
        );
        let c = Controller::read(&qemu_controller());
        assert_eq!(
            (c.vendor, c.serial(), c.model()),
            (0x1b36, "meridian", "QEMU NVMe Ctrl")
        );
        assert_eq!(c.namespaces, 256);
    }

    #[test]
    fn qemus_namespace_reads_as_the_test_image() {
        let n = Namespace::read(&qemu_namespace()).unwrap();
        assert_eq!(
            (n.blocks, n.block_size),
            (135_168, 512),
            "66 MiB of 512-byte blocks"
        );
        let mut four_k = qemu_namespace();
        four_k[26] = 4;
        assert_eq!(
            Namespace::read(&four_k).map(|n| n.block_size),
            Some(4096),
            "its fifth format"
        );
        four_k[26] = 3;
        assert_eq!(
            Namespace::read(&four_k).map(|n| n.block_size),
            Some(512),
            "the fourth, with metadata"
        );
    }

    #[test]
    fn every_command_leaves_what_it_does_not_use_at_zero() {
        let commands = [
            identify(1, 0, 0x1000),
            identify(2, 1, 0x1000),
            create_completion_queue(3, 1, 64, 0x2000),
            create_submission_queue(4, 1, 64, 0x3000, 1),
            read(5, 1, 7, 8, 0x4000).unwrap(),
        ];
        for c in commands {
            assert_eq!(c[1], 0, "no fused operation, PRPs for data: {c:x?}");
            assert!(c[8..24].iter().all(|&b| b == 0), "no metadata pointer");
            assert!(
                c[32..40].iter().all(|&b| b == 0),
                "one page, no second pointer"
            );
            assert!(c[52..64].iter().all(|&b| b == 0), "the last three words");
        }
    }

    #[test]
    fn the_status_says_whether_the_controller_came_up() {
        assert_eq!(State::read(0), State::Off);
        assert_eq!(State::read(1), State::Ready);
        assert_eq!(State::read(1 | 0b1100), State::Ready, "while it shuts down");
        assert_eq!(State::read(0b10), State::Failed);
        assert_eq!(
            State::read(0b11),
            State::Failed,
            "failed, though it says it is ready"
        );
        assert_eq!(State::read(0b1100), State::Off);
    }

    #[test]
    fn meridian_drives_a_controller_only_as_it_can() {
        let qemu = Capabilities::read(QEMU_CAPABILITIES);
        assert!(qemu.usable(64, 0x2000));
        assert!(
            qemu.usable(2048, 0x1010),
            "the largest queues, the doorbells just fitting"
        );
        assert!(!qemu.usable(2049, 0x2000), "queues larger than it takes");
        assert!(!qemu.usable(64, 0x100f), "the last doorbell one byte short");
        let wide = Capabilities::read(QEMU_CAPABILITIES | 8 << 32);
        assert_eq!(wide.doorbell(1, true), 0x1c00);
        assert!(wide.usable(64, 0x2000));
        assert!(!Capabilities::read(QEMU_CAPABILITIES | 9 << 32).usable(64, 0x2000));
        assert!(
            !Capabilities::read(QEMU_CAPABILITIES | 1 << 48).usable(64, 0x2000),
            "8 KiB pages"
        );
        assert!(
            !Capabilities::read(QEMU_CAPABILITIES & !(1 << 37)).usable(64, 0x2000),
            "no NVM commands"
        );
    }

    #[test]
    fn capabilities_say_what_the_controller_can() {
        let qemu = 0x7ff | 1 << 16 | 0xf << 24 | 1 << 37 | 4 << 52;
        let c = Capabilities::read(qemu);
        assert_eq!(c.queue_entries, 2048);
        assert_eq!(c.doorbell_stride, 4);
        assert_eq!(c.timeout_ms, 7500);
        assert!(c.nvm);
        assert_eq!(c.page_min, 4096);
        let other = Capabilities::read(0x3f | 2 << 32 | 1 << 48 | 0xffff_u64 << 56);
        assert_eq!(
            (
                other.queue_entries,
                other.doorbell_stride,
                other.nvm,
                other.page_min
            ),
            (64, 16, false, 8192)
        );
        assert_eq!(
            Capabilities::read(0xffff).queue_entries,
            65_536,
            "the largest a queue gets"
        );
    }

    #[test]
    fn each_queue_has_two_doorbells() {
        let narrow = Capabilities::read(1 << 37);
        assert_eq!(narrow.doorbell(0, false), 0x1000);
        assert_eq!(narrow.doorbell(0, true), 0x1004);
        assert_eq!(narrow.doorbell(1, false), 0x1008);
        assert_eq!(narrow.doorbell(1, true), 0x100c);
        let wide = Capabilities::read(2 << 32);
        assert_eq!(wide.doorbell(1, true), 0x1030, "16 bytes apart");
    }

    #[test]
    fn the_controller_is_turned_on_for_64_and_16_byte_entries_and_4_kib_pages() {
        assert_eq!(ENABLED, 0x0046_0001);
        assert_eq!(ENABLED & 1, 1, "enabled");
        assert_eq!(ENABLED >> 4 & 0x7, 0, "the NVM command set");
        assert_eq!(ENABLED >> 7 & 0xf, 0, "pages of 4 KiB");
        assert_eq!(
            (ENABLED >> 16 & 0xf, ENABLED >> 20 & 0xf),
            (6, 4),
            "2 to the 6 and 2 to the 4 bytes"
        );
        assert_eq!(admin_queues(64), 0x003f_003f);
        assert_eq!(admin_queues(0), 0x0001_0001, "no fewer than two entries");
        assert_eq!(admin_queues(4096), 0x0fff_0fff);
    }

    #[test]
    fn identify_asks_for_the_controller_or_a_namespace() {
        let controller = identify(7, 0, 0x1234_5000);
        assert_eq!(controller[0], opcode::IDENTIFY);
        assert_eq!(&controller[2..4], &7u16.to_le_bytes(), "the command's id");
        assert_eq!(word(&controller, 4), 0);
        assert_eq!(
            &controller[24..32],
            &0x1234_5000u64.to_le_bytes(),
            "where the answer goes"
        );
        assert_eq!(word(&controller, 40), 1, "the controller's data");
        let namespace = identify(8, 1, 0x1_0000_0000);
        assert_eq!(
            (word(&namespace, 4), word(&namespace, 40)),
            (1, 0),
            "namespace 1's data"
        );
        assert_eq!(&namespace[24..32], &0x1_0000_0000u64.to_le_bytes());
        assert!(
            namespace[32..40].iter().all(|&b| b == 0),
            "one page needs no second pointer"
        );
    }

    #[test]
    fn queues_are_made_contiguous_and_without_interrupts() {
        let cq = create_completion_queue(1, 1, 256, 0x7000);
        assert_eq!(cq[0], opcode::CREATE_COMPLETION_QUEUE);
        assert_eq!(word(&cq, 40), 255 << 16 | 1);
        assert_eq!(word(&cq, 44), 1, "contiguous, no interrupts");
        assert_eq!(&cq[24..32], &0x7000u64.to_le_bytes());
        let sq = create_submission_queue(2, 1, 64, 0x8000, 1);
        assert_eq!(sq[0], opcode::CREATE_SUBMISSION_QUEUE);
        assert_eq!(word(&sq, 40), 63 << 16 | 1);
        assert_eq!(word(&sq, 44), 1 << 16 | 1, "completing in queue 1");
        assert_eq!(
            word(&create_submission_queue(3, 2, 1, 0, 9), 40),
            1 << 16 | 2,
            "two entries at least"
        );
    }

    #[test]
    fn a_read_names_its_blocks_counted_from_0() {
        let r = read(5, 1, 0x1_2345_6789, 8, 0x9000).unwrap();
        assert_eq!(r[0], opcode::READ);
        assert_eq!(word(&r, 4), 1);
        assert_eq!(
            (word(&r, 40), word(&r, 44)),
            (0x2345_6789, 1),
            "the first block in two words"
        );
        assert_eq!(word(&r, 48), 7, "eight blocks");
        assert_eq!(read(5, 1, 0, 0, 0), None, "no blocks");
        assert_eq!(read(5, 1, 0, 65_536, 0).map(|c| word(&c, 48)), Some(0xffff));
        assert_eq!(read(5, 1, 0, 65_537, 0), None, "more than a count holds");
    }

    #[test]
    fn a_completion_says_what_it_answers_and_how() {
        let mut entry = [0u8; 16];
        entry[8..10].copy_from_slice(&3u16.to_le_bytes());
        entry[12..16].copy_from_slice(&(0x2a_u32 | 1 << 16).to_le_bytes());
        assert_eq!(
            Completion::read(&entry),
            Completion {
                head: 3,
                id: 42,
                phase: true,
                status: 0
            }
        );
        entry[12..16].copy_from_slice(&(7u32 | 0x0281 << 17).to_le_bytes());
        let failed = Completion::read(&entry);
        assert_eq!((failed.id, failed.phase, failed.status), (7, false, 0x0281));
        entry[12..16].copy_from_slice(&(7u32 | 1 << 16 | 0x0281 << 17).to_le_bytes());
        let failed = Completion::read(&entry);
        assert_eq!(
            (failed.phase, failed.status),
            (true, 0x0281),
            "the phase next to a status"
        );
        assert_eq!(Completion::read(&[0xff; 16]).status, 0x7fff);
    }

    /// IDENTIFY's answer for a controller.
    fn controller(vendor: u16, serial: &str, model: &str, namespaces: u32) -> [u8; 4096] {
        let mut data = [0u8; 4096];
        data[0..2].copy_from_slice(&vendor.to_le_bytes());
        data[4..24].copy_from_slice(format!("{serial:<20}").as_bytes());
        data[24..64].copy_from_slice(format!("{model:<40}").as_bytes());
        data[516..520].copy_from_slice(&namespaces.to_le_bytes());
        data
    }

    #[test]
    fn a_controller_says_who_made_it() {
        let c = Controller::read(&controller(0x1b36, "meridian", "QEMU NVMe Ctrl", 256));
        assert_eq!(
            (c.vendor, c.serial(), c.model(), c.namespaces),
            (0x1b36, "meridian", "QEMU NVMe Ctrl", 256)
        );
        let full = "S".repeat(20);
        assert_eq!(
            Controller::read(&controller(1, &full, "M", 1)).serial(),
            full
        );
        assert_eq!(Controller::read(&[0; 4096]).model(), "");
    }

    /// IDENTIFY's answer for a namespace of `blocks`, its format `format`
    /// one of `2 ^ shift` bytes.
    fn namespace(blocks: u64, format: u8, shift: u8) -> [u8; 4096] {
        let mut data = [0u8; 4096];
        data[0..8].copy_from_slice(&blocks.to_le_bytes());
        data[26] = format;
        data[128 + 4 * usize::from(format & 0xf) + 2] = shift;
        data
    }

    #[test]
    fn a_namespace_says_how_many_blocks_of_what_size() {
        assert_eq!(
            Namespace::read(&namespace(135_168, 0, 9)),
            Some(Namespace {
                blocks: 135_168,
                block_size: 512
            })
        );
        assert_eq!(
            Namespace::read(&namespace(1 << 40, 1, 12)).map(|n| (n.blocks, n.block_size)),
            Some((1 << 40, 4096))
        );
        assert_eq!(
            Namespace::read(&namespace(8, 0x13, 9)).map(|n| n.block_size),
            Some(512),
            "only the low four bits pick the format"
        );
        assert_eq!(Namespace::read(&namespace(0, 0, 9)), None, "no blocks");
        assert_eq!(Namespace::read(&namespace(8, 0, 8)), None, "256 bytes");
        assert_eq!(Namespace::read(&namespace(8, 0, 17)), None, "128 KiB");
        assert!(Namespace::read(&namespace(8, 0, 16)).is_some());
    }
}
