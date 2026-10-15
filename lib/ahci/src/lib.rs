//! AHCI, the parts that need no hardware (Serial ATA AHCI 1.3.1): where
//! the registers are, the frames a command is made of, what a port says
//! is attached to it, and what a disk answers to IDENTIFY DEVICE. The
//! kernel writes and reads the registers and the memory the controller
//! reads from; this crate says what goes there and what came back.

#![cfg_attr(not(test), no_std)]

/// The registers every controller has, by their offset from its base.
pub mod hba {
    pub const CAPABILITIES: usize = 0x00;
    pub const CONTROL: usize = 0x04;
    pub const PORTS: usize = 0x0c;
    pub const VERSION: usize = 0x10;
    /// In `CONTROL`: the controller speaks AHCI, not only IDE.
    pub const AHCI_ENABLE: u32 = 1 << 31;
}

/// The registers of each port, by their offset from its own base.
pub mod port {
    /// Where the registers of port `n` start.
    #[must_use]
    pub const fn base(n: usize) -> usize {
        0x100 + 0x80 * n
    }

    pub const COMMAND_LIST: usize = 0x00;
    pub const COMMAND_LIST_HIGH: usize = 0x04;
    pub const RECEIVED: usize = 0x08;
    pub const RECEIVED_HIGH: usize = 0x0c;
    pub const INTERRUPTS: usize = 0x10;
    pub const COMMAND: usize = 0x18;
    pub const TASK_FILE: usize = 0x20;
    pub const SIGNATURE: usize = 0x24;
    pub const STATUS: usize = 0x28;
    pub const ERRORS: usize = 0x30;
    pub const ISSUE: usize = 0x38;

    /// In `COMMAND`: start running commands, take in what the device
    /// sends, and whether either still runs.
    pub const START: u32 = 1;
    pub const RECEIVE: u32 = 1 << 4;
    pub const RECEIVING: u32 = 1 << 14;
    pub const RUNNING: u32 = 1 << 15;
    /// In `TASK_FILE`: the device is busy, wants data, or failed.
    pub const BUSY: u32 = 1 << 7;
    pub const DATA_REQUEST: u32 = 1 << 3;
    pub const FAILED: u32 = 1;
    /// In `INTERRUPTS`: the device reported an error in its task file.
    pub const TASK_FILE_ERROR: u32 = 1 << 30;
}

/// The ports a controller has, in order, from its register of them.
pub fn ports(implemented: u32) -> impl Iterator<Item = usize> {
    (0..32).filter(move |&n| implemented >> n & 1 == 1)
}

/// The low and the high half of `address`, for the two registers that
/// hold a 64-bit address.
#[must_use]
pub fn halves(address: u64) -> (u32, u32) {
    let half = |bits: u64| u32::try_from(bits & 0xffff_ffff).unwrap_or(0);
    (half(address), half(address >> 32))
}

/// The size of a sector, unless a disk says otherwise.
pub const SECTOR: u32 = 512;

/// The first sector a 48-bit address cannot reach.
pub const LBA_END: u64 = 1 << 48;

/// What is attached to a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attached {
    Nothing,
    Disk,
    /// A drive that speaks ATAPI, such as a DVD drive.
    Optical,
    /// A port multiplier or something else, by its signature.
    Other(u32),
}

/// What a port's SATA status and signature say is attached: a device is
/// there only when it is detected with the link up and is not asleep.
#[must_use]
pub fn attached(status: u32, signature: u32) -> Attached {
    let detected = status & 0xf == 3;
    let awake = status >> 8 & 0xf == 1;
    if !detected || !awake {
        return Attached::Nothing;
    }
    match signature {
        0x0000_0101 => Attached::Disk,
        0xeb14_0101 => Attached::Optical,
        other => Attached::Other(other),
    }
}

/// The ATA commands Meridian sends.
pub mod ata {
    /// Does nothing and fails, as ATA says it always does.
    pub const NOP: u8 = 0x00;
    pub const IDENTIFY: u8 = 0xec;
    pub const READ_DMA_EXT: u8 = 0x25;
    pub const WRITE_DMA_EXT: u8 = 0x35;
}

/// A register FIS from host to device carrying `command` for `count`
/// sectors from `lba` on: the 20 bytes at the start of a command table.
/// None when the sectors reach past what 48 bits address, where the
/// address would wrap around to another sector.
#[must_use]
pub fn command_fis(command: u8, lba: u64, count: u16) -> Option<[u8; 20]> {
    if lba
        .checked_add(u64::from(count))
        .is_none_or(|end| end > LBA_END)
    {
        return None;
    }
    let lba = lba.to_le_bytes();
    let count = count.to_le_bytes();
    let mut fis = [0u8; 20];
    fis[0] = 0x27;
    fis[1] = 0x80;
    fis[2] = command;
    fis[4..7].copy_from_slice(&lba[..3]);
    fis[7] = 0x40;
    fis[8..11].copy_from_slice(&lba[3..6]);
    fis[12..14].copy_from_slice(&count);
    Some(fis)
}

/// A command header: a FIS of 5 double words, whether data goes to the
/// device, how many PRDT entries the table at `table` has, and the table.
#[must_use]
pub fn command_header(write: bool, entries: u16, table: u64) -> [u8; 32] {
    let flags = 5 | if write { 1 << 6 } else { 0 } | u32::from(entries) << 16;
    let mut header = [0u8; 32];
    header[0..4].copy_from_slice(&flags.to_le_bytes());
    header[8..16].copy_from_slice(&table.to_le_bytes());
    header
}

/// The most bytes one PRDT entry moves.
pub const ENTRY_MAX: u32 = 4 << 20;

/// A PRDT entry for the `bytes` at physical `address`; none for a count
/// the controller cannot move: 0, odd, or more than [`ENTRY_MAX`].
#[must_use]
pub fn prdt_entry(address: u64, bytes: u32) -> Option<[u8; 16]> {
    if bytes == 0 || bytes % 2 == 1 || bytes > ENTRY_MAX {
        return None;
    }
    let mut entry = [0u8; 16];
    entry[0..8].copy_from_slice(&address.to_le_bytes());
    entry[12..16].copy_from_slice(&(bytes - 1).to_le_bytes());
    Some(entry)
}

/// What a disk says about itself in its 512 bytes of IDENTIFY data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    model: [u8; 40],
    pub sectors: u64,
    pub sector_size: u32,
}

impl Identity {
    #[must_use]
    pub fn read(data: &[u8; 512]) -> Identity {
        let word = |n: usize| u16::from_le_bytes([data[2 * n], data[2 * n + 1]]);
        let mut model = [0u8; 40];
        for (i, pair) in model.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            *pair = word(27 + i).to_be_bytes();
        }
        let lba48 = word(83) & 1 << 10 != 0;
        let sectors = if lba48 {
            (0..4).map(|i| u64::from(word(100 + i)) << (16 * i)).sum()
        } else {
            u64::from(word(60)) | u64::from(word(61)) << 16
        };
        let words_per_sector = u32::from(word(117)) | u32::from(word(118)) << 16;
        let sector_size = if word(106) & 0xc000 == 0x4000 && word(106) & 1 << 12 != 0 {
            2 * words_per_sector
        } else {
            SECTOR
        };
        Identity {
            model,
            sectors,
            sector_size,
        }
    }

    /// The model, without the spaces the disk pads it with.
    #[must_use]
    pub fn model(&self) -> &str {
        core::str::from_utf8(&self.model)
            .unwrap_or("")
            .trim_end_matches([' ', '\0'])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bits_are_where_the_specification_puts_them() {
        assert_eq!(hba::AHCI_ENABLE, 0x8000_0000);
        assert_eq!(
            [port::START, port::RECEIVE, port::RECEIVING, port::RUNNING],
            [0x1, 0x10, 0x4000, 0x8000]
        );
        assert_eq!(
            [port::BUSY, port::DATA_REQUEST, port::FAILED],
            [0x80, 0x08, 0x01]
        );
        assert_eq!(port::TASK_FILE_ERROR, 0x4000_0000);
        assert_eq!(
            [
                port::COMMAND,
                port::TASK_FILE,
                port::SIGNATURE,
                port::STATUS,
                port::ISSUE
            ],
            [0x18, 0x20, 0x24, 0x28, 0x38]
        );
    }

    #[test]
    fn port_registers_follow_one_another() {
        assert_eq!(port::base(0), 0x100);
        assert_eq!(port::base(1), 0x180);
        assert_eq!(port::base(31), 0x1080, "the last of 32");
        assert_eq!(port::base(31) + port::ISSUE + 4, 0x10bc, "all within 8 KiB");
    }

    #[test]
    fn only_the_ports_a_controller_has_are_looked_at() {
        let qemu: Vec<usize> = ports(0x3f).collect();
        assert_eq!(qemu, [0, 1, 2, 3, 4, 5], "the six of q35's controller");
        assert_eq!(ports(0x8000_0002).collect::<Vec<_>>(), [1, 31]);
        assert_eq!(ports(u32::MAX).count(), 32);
        assert_eq!(ports(0).count(), 0);
    }

    #[test]
    fn an_address_is_split_in_halves() {
        assert_eq!(halves(0x1234_5678_9abc_def0), (0x9abc_def0, 0x1234_5678));
        assert_eq!(halves(0x7ffd_f000), (0x7ffd_f000, 0), "a frame below 4 GiB");
        assert_eq!(halves(u64::MAX), (u32::MAX, u32::MAX));
    }

    #[test]
    fn a_disk_is_there_when_its_link_is_up_and_it_is_awake() {
        assert_eq!(attached(0x113, 0x0000_0101), Attached::Disk);
        assert_eq!(
            attached(0x123, 0x0000_0101),
            Attached::Disk,
            "at another speed"
        );
        assert_eq!(attached(0x113, 0xeb14_0101), Attached::Optical);
        assert_eq!(
            attached(0x113, 0x9669_0101),
            Attached::Other(0x9669_0101),
            "a port multiplier"
        );
        assert_eq!(attached(0x000, 0x0000_0101), Attached::Nothing, "no device");
        assert_eq!(
            attached(0x111, 0x0000_0101),
            Attached::Nothing,
            "found but no link"
        );
        assert_eq!(attached(0x213, 0x0000_0101), Attached::Nothing, "asleep");
        assert_eq!(
            attached(0x013, 0x0000_0101),
            Attached::Nothing,
            "no power state"
        );
        assert_eq!(
            attached(0x114, 0x0000_0101),
            Attached::Nothing,
            "the link is off"
        );
    }

    #[test]
    fn a_read_names_its_sectors_in_48_bits() {
        let fis = command_fis(ata::READ_DMA_EXT, 0x0000_1234_5678_9abc, 8).unwrap();
        assert_eq!(&fis[..4], &[0x27, 0x80, 0x25, 0], "a command from the host");
        assert_eq!(
            &fis[4..7],
            &[0xbc, 0x9a, 0x78],
            "the low half of the address"
        );
        assert_eq!(fis[7], 0x40, "by address, not by cylinder");
        assert_eq!(&fis[8..11], &[0x56, 0x34, 0x12], "the high half");
        assert_eq!(&fis[12..14], &[8, 0]);
        assert!(fis[14..].iter().all(|&b| b == 0) && fis[3] == 0 && fis[11] == 0);
        let identify = command_fis(ata::IDENTIFY, 0, 0).unwrap();
        assert_eq!(&identify[..4], &[0x27, 0x80, 0xec, 0]);
        assert_eq!(
            command_fis(ata::WRITE_DMA_EXT, 1, 0x100).unwrap()[2..14],
            [0x35, 0, 1, 0, 0, 0x40, 0, 0, 0, 0, 0, 1]
        );
    }

    #[test]
    fn no_command_reaches_past_48_bits() {
        let last = command_fis(ata::READ_DMA_EXT, LBA_END - 1, 1).unwrap();
        assert_eq!(&last[4..7], &[0xff; 3]);
        assert_eq!(&last[8..11], &[0xff; 3], "the last sector there is");
        assert!(
            command_fis(ata::READ_DMA_EXT, LBA_END - 8, 8).is_some(),
            "ending right at the end"
        );
        assert_eq!(
            command_fis(ata::READ_DMA_EXT, LBA_END - 8, 9),
            None,
            "one past it"
        );
        assert_eq!(
            command_fis(ata::READ_DMA_EXT, LBA_END, 1),
            None,
            "an address that would wrap to 0"
        );
        assert_eq!(command_fis(ata::READ_DMA_EXT, u64::MAX, 2), None);
        assert!(command_fis(ata::READ_DMA_EXT, 0, u16::MAX).is_some());
    }

    #[test]
    fn a_header_says_how_the_command_is_built() {
        let read = command_header(false, 1, 0x1234_5000);
        assert_eq!(
            u32::from_le_bytes(read[0..4].try_into().unwrap()),
            0x0001_0005
        );
        assert_eq!(
            u64::from_le_bytes(read[8..16].try_into().unwrap()),
            0x1234_5000
        );
        assert!(
            read[4..8].iter().chain(&read[16..]).all(|&b| b == 0),
            "nothing moved yet"
        );
        let write = command_header(true, 3, 0x1_0000_0080);
        assert_eq!(
            u32::from_le_bytes(write[0..4].try_into().unwrap()),
            0x0003_0045
        );
        assert_eq!(
            u64::from_le_bytes(write[8..16].try_into().unwrap()),
            0x1_0000_0080
        );
    }

    #[test]
    fn a_header_has_room_for_every_entry_and_any_address() {
        let most = command_header(false, u16::MAX, 0xffff_ffff_ffff_ff80);
        assert_eq!(
            u32::from_le_bytes(most[0..4].try_into().unwrap()),
            0xffff_0005
        );
        assert_eq!(&most[8..16], &0xffff_ffff_ffff_ff80u64.to_le_bytes());
        let none = command_header(true, 0, 0);
        assert_eq!(
            u32::from_le_bytes(none[0..4].try_into().unwrap()),
            0x45,
            "no entries, no data"
        );
        let high = prdt_entry(0xffff_ffff_0000_0000, 512).unwrap();
        assert_eq!(
            &high[0..8],
            &0xffff_ffff_0000_0000u64.to_le_bytes(),
            "memory past 4 GiB"
        );
        assert_eq!(&high[12..16], &511u32.to_le_bytes());
    }

    #[test]
    fn an_entry_moves_an_even_number_of_bytes_up_to_4_mib() {
        let entry = prdt_entry(0x8_7654_3000, 4096).unwrap();
        assert_eq!(
            u64::from_le_bytes(entry[0..8].try_into().unwrap()),
            0x8_7654_3000
        );
        assert_eq!(
            u32::from_le_bytes(entry[12..16].try_into().unwrap()),
            4095,
            "counted from 0"
        );
        assert!(entry[8..12].iter().all(|&b| b == 0));
        assert!(prdt_entry(0, 2).is_some());
        assert!(prdt_entry(0, ENTRY_MAX).is_some());
        assert_eq!(prdt_entry(0, ENTRY_MAX + 2), None);
        assert_eq!(prdt_entry(0, 0), None);
        assert_eq!(prdt_entry(0, 511), None);
    }

    /// IDENTIFY data with `model`, and the sector count either in the 28
    /// bit words or, with LBA48, in the 48 bit ones.
    fn identify(model: &str, sectors: u64, lba48: bool) -> [u8; 512] {
        let mut data = [0u8; 512];
        let mut put = |word: usize, value: u16| {
            data[2 * word..2 * word + 2].copy_from_slice(&value.to_le_bytes());
        };
        let padded = format!("{model:<40}");
        for (i, pair) in padded.as_bytes().chunks(2).enumerate() {
            put(27 + i, u16::from_be_bytes([pair[0], pair[1]]));
        }
        if lba48 {
            put(83, 1 << 10);
            for i in 0..4 {
                put(
                    100 + i,
                    u16::try_from(sectors >> (16 * i) & 0xffff).unwrap(),
                );
            }
        } else {
            put(60, u16::try_from(sectors & 0xffff).unwrap());
            put(61, u16::try_from(sectors >> 16 & 0xffff).unwrap());
        }
        data
    }

    #[test]
    fn a_disk_says_its_model_and_size() {
        let qemu = Identity::read(&identify("QEMU HARDDISK", 0x2_0000, true));
        assert_eq!(qemu.model(), "QEMU HARDDISK");
        assert_eq!(qemu.sectors, 0x2_0000);
        assert_eq!(qemu.sector_size, 512);
        let big = Identity::read(&identify("Samsung SSD 990 PRO 2TB", 3_907_029_168, true));
        assert_eq!(big.model(), "Samsung SSD 990 PRO 2TB");
        assert_eq!(big.sectors, 3_907_029_168, "past 32 bits");
        let old = Identity::read(&identify("OLD DISK", 0x0fff_ffff, false));
        assert_eq!(
            old.sectors, 0x0fff_ffff,
            "without LBA48, from the 28-bit words"
        );
        assert_eq!(Identity::read(&[0; 512]).model(), "");
        let mut both = identify("BOTH", 0x1_0000_0000, true);
        both[120..124].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
        assert_eq!(
            Identity::read(&both).sectors,
            0x1_0000_0000,
            "with LBA48 the 48-bit count wins over the old one"
        );
        let full = "A".repeat(40);
        assert_eq!(
            Identity::read(&identify(&full, 1, true)).model(),
            full,
            "40 characters, no padding"
        );
        let mut zeros = identify("PADDED", 1, true);
        zeros[54 + 10..54 + 40].fill(0);
        assert_eq!(
            Identity::read(&zeros).model(),
            "PADDED",
            "zero bytes pad it too"
        );
    }

    #[test]
    fn a_disk_with_large_sectors_says_how_large() {
        let mut data = identify("4K DISK", 1000, true);
        data[212..214].copy_from_slice(&(0x4000u16 | 1 << 12).to_le_bytes());
        data[234..238].copy_from_slice(&2048u32.to_le_bytes());
        assert_eq!(Identity::read(&data).sector_size, 4096);
        data[212..214].copy_from_slice(&(1u16 << 12).to_le_bytes());
        assert_eq!(Identity::read(&data).sector_size, 512, "word 106 not valid");
        data[212..214].copy_from_slice(&0x4000u16.to_le_bytes());
        assert_eq!(
            Identity::read(&data).sector_size,
            512,
            "valid, but no large sectors"
        );
        data[212..214].copy_from_slice(&(0x4000u16 | 1 << 12).to_le_bytes());
        data[234..238].copy_from_slice(&0x1_0000u32.to_le_bytes());
        assert_eq!(
            Identity::read(&data).sector_size,
            0x2_0000,
            "128 KiB, which takes both words"
        );
    }
}
