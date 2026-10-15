//! Reads the ACPI tables the firmware leaves in memory (ACPI 6.5): the
//! root pointer, the extended system description table that lists the
//! others, the MADT with the processors, the FADT with the power
//! management timer, the HPET table, and the MCFG with where PCI's
//! configuration space lies. Memory is reached through a [`Memory`] the caller
//! provides, so the same code reads the real tables in the kernel and
//! tables built byte by byte in the tests. Every table's checksum is
//! checked before anything in it is believed.

#![cfg_attr(not(test), no_std)]

/// Physical memory as the tables are read from it.
pub trait Memory {
    /// The `len` bytes at physical address `at`, or `None` where there is
    /// no memory to read.
    fn bytes(&self, at: u64, len: usize) -> Option<&[u8]>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes do not start with the signature they should.
    Signature,
    /// The bytes do not add up to 0, as every ACPI table must.
    Checksum,
    /// A length or address points where there is nothing to read.
    Truncated,
    /// The root pointer is from ACPI 1.0, which has no XSDT.
    TooOld,
    /// The XSDT lists no table with this signature.
    Missing([u8; 4]),
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b)) == 0
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The address of the XSDT, read from the root pointer at `rsdp`.
///
/// # Errors
///
/// When the root pointer's signature, checksums or revision are wrong.
pub fn xsdt_address(memory: &impl Memory, rsdp: u64) -> Result<u64, Error> {
    let head = memory.bytes(rsdp, 20).ok_or(Error::Truncated)?;
    if &head[..8] != b"RSD PTR " {
        return Err(Error::Signature);
    }
    if !checksum_ok(head) {
        return Err(Error::Checksum);
    }
    if head[15] < 2 {
        return Err(Error::TooOld);
    }
    let whole = memory.bytes(rsdp, 36).ok_or(Error::Truncated)?;
    let len = u32_at(whole, 20).ok_or(Error::Truncated)? as usize;
    let all = memory.bytes(rsdp, len.max(36)).ok_or(Error::Truncated)?;
    if !checksum_ok(all) {
        return Err(Error::Checksum);
    }
    u64_at(whole, 24).ok_or(Error::Truncated)
}

/// The whole table at `at` with the four letter `signature`, checked.
///
/// # Errors
///
/// When the signature or checksum is wrong, or the table is cut short.
pub fn table<'m>(memory: &'m impl Memory, at: u64, signature: &[u8; 4]) -> Result<&'m [u8], Error> {
    let header = memory.bytes(at, 36).ok_or(Error::Truncated)?;
    if &header[..4] != signature {
        return Err(Error::Signature);
    }
    let len = u32_at(header, 4).ok_or(Error::Truncated)? as usize;
    if len < 36 {
        return Err(Error::Truncated);
    }
    let all = memory.bytes(at, len).ok_or(Error::Truncated)?;
    if !checksum_ok(all) {
        return Err(Error::Checksum);
    }
    Ok(all)
}

/// The address of the first table in the XSDT at `xsdt` with `signature`.
///
/// # Errors
///
/// [`Error::Missing`] when no listed table has the signature, and the
/// errors of [`table`] for the XSDT itself.
pub fn find(memory: &impl Memory, xsdt: u64, signature: &[u8; 4]) -> Result<u64, Error> {
    let list = table(memory, xsdt, b"XSDT")?;
    list[36..]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|entry| u64::from_le_bytes(*entry))
        .find(|&at| memory.bytes(at, 4).is_some_and(|s| s == signature))
        .ok_or(Error::Missing(*signature))
}

/// What the MADT says about the processors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Processors {
    /// Processors that are enabled, or can be brought online.
    pub usable: u32,
    /// All processor entries, usable or not.
    pub listed: u32,
    /// Where each processor's local APIC is mapped.
    pub local_apic: u64,
}

/// One processor the MADT lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processor {
    /// The id of its local APIC, which interrupts sent to it name.
    pub apic_id: u32,
    /// Whether it is enabled or can be brought online.
    pub usable: bool,
}

/// Calls `each` with every processor in the MADT, from its xAPIC and its
/// x2APIC entries alike, in the table's order.
///
/// # Errors
///
/// [`Error::Truncated`] when an entry's length points past the table or
/// is 0, which would never end.
pub fn each_processor(madt: &[u8], mut each: impl FnMut(Processor)) -> Result<(), Error> {
    walk_madt(madt, |kind, entry| {
        let (id, flags) = match kind {
            0 => (entry.get(3).map(|&id| u32::from(id)), entry.get(4..8)),
            9 => (u32_at(entry, 4), entry.get(8..12)),
            _ => return Ok(()),
        };
        let (Some(apic_id), Some(flags)) = (id, flags) else {
            return Err(Error::Truncated);
        };
        let flags = u32::from_le_bytes(flags.try_into().map_err(|_| Error::Truncated)?);
        each(Processor {
            apic_id,
            usable: flags & 0b11 != 0,
        });
        Ok(())
    })
}

/// Calls `entry` with the type and bytes of every entry of the MADT.
fn walk_madt(
    madt: &[u8],
    mut entry: impl FnMut(u8, &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut at = 44;
    while at < madt.len() {
        let len = usize::from(*madt.get(at + 1).ok_or(Error::Truncated)?);
        let bytes = madt
            .get(at..at + len)
            .filter(|_| len >= 2)
            .ok_or(Error::Truncated)?;
        entry(madt[at], bytes)?;
        at += len;
    }
    Ok(())
}

/// Counts the processors in the MADT, both their xAPIC and x2APIC entries.
///
/// # Errors
///
/// [`Error::Truncated`] when an entry's length points past the table or
/// is 0, which would never end.
pub fn processors(madt: &[u8]) -> Result<Processors, Error> {
    let mut found = Processors {
        local_apic: u64::from(u32_at(madt, 36).ok_or(Error::Truncated)?),
        ..Processors::default()
    };
    walk_madt(madt, |kind, entry| {
        if kind == 5 {
            found.local_apic = u64_at(entry, 4).ok_or(Error::Truncated)?;
        }
        Ok(())
    })?;
    each_processor(madt, |p| {
        found.listed += 1;
        found.usable += u32::from(p.usable);
    })?;
    Ok(found)
}

/// The I/O port of the power management timer, which counts at
/// 3.579545 MHz whatever the processor does, from the FADT. `None` on
/// machines without one.
#[must_use]
pub fn pm_timer_port(fadt: &[u8]) -> Option<u16> {
    // The extended block at 208: address space 1 is I/O. Older FADTs only
    // have the 32-bit port at 76.
    if let (Some(&1), Some(address)) = (fadt.get(208), u64_at(fadt, 212))
        && address != 0
    {
        return u16::try_from(address).ok();
    }
    u32_at(fadt, 76)
        .filter(|&p| p != 0)
        .and_then(|p| u16::try_from(p).ok())
}

/// How many bits the power management timer counts in: 32 when the
/// FADT's flags say so, 24 otherwise.
#[must_use]
pub fn pm_timer_bits(fadt: &[u8]) -> u32 {
    if u32_at(fadt, 112).is_some_and(|flags| flags & 1 << 8 != 0) {
        32
    } else {
        24
    }
}

/// The frequency of the power management timer, in ticks per second.
pub const PM_TIMER_HZ: u64 = 3_579_545;

/// Where the High Precision Event Timer's registers are, from the HPET
/// table: its base address, a generic address structure at 40, whose
/// address space 0 is memory. `None` for a table that puts them anywhere
/// else, which the HPET specification does not allow, or is too short.
#[must_use]
pub fn hpet_address(hpet: &[u8]) -> Option<u64> {
    if hpet.get(40) != Some(&0) {
        return None;
    }
    u64_at(hpet, 44).filter(|&address| address != 0)
}

/// Where the configuration space of a range of PCI buses lies in memory,
/// as the PCI Express enhanced configuration access mechanism (ECAM) has
/// it: 4 KiB for each function, 1 MiB for each bus from `first_bus` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ecam {
    pub base: u64,
    pub segment: u16,
    pub first_bus: u8,
    pub last_bus: u8,
}

/// The ECAM regions the MCFG lists: entries of 16 bytes from byte 44 on,
/// as far as the table goes.
pub fn ecam_regions(mcfg: &[u8]) -> impl Iterator<Item = Ecam> + '_ {
    let len = u32_at(mcfg, 4).map_or(0, |l| l as usize).min(mcfg.len());
    mcfg.get(44..len)
        .unwrap_or_default()
        .as_chunks::<16>()
        .0
        .iter()
        .map(|entry| Ecam {
            base: u64::from_le_bytes([
                entry[0], entry[1], entry[2], entry[3], entry[4], entry[5], entry[6], entry[7],
            ]),
            segment: u16::from_le_bytes([entry[8], entry[9]]),
            first_bus: entry[10],
            last_bus: entry[11],
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Physical memory from `base` on, holding `bytes`.
    struct Ram {
        base: u64,
        bytes: Vec<u8>,
    }

    impl Memory for Ram {
        fn bytes(&self, at: u64, len: usize) -> Option<&[u8]> {
            let start = usize::try_from(at.checked_sub(self.base)?).ok()?;
            self.bytes.get(start..start.checked_add(len)?)
        }
    }

    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        bytes[at] = sum.wrapping_neg();
    }

    /// A table with the standard 36 byte header and a right checksum.
    fn sdt(signature: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; 36];
        t[..4].copy_from_slice(&signature);
        t[4..8].copy_from_slice(&u32::try_from(36 + body.len()).unwrap().to_le_bytes());
        t[8] = 6;
        t[10..16].copy_from_slice(b"MERIDN");
        t.extend_from_slice(body);
        fix_checksum(&mut t, 9);
        t
    }

    fn rsdp(xsdt: u64, revision: u8) -> Vec<u8> {
        let mut r = vec![0u8; 36];
        r[..8].copy_from_slice(b"RSD PTR ");
        r[9..15].copy_from_slice(b"MERIDN");
        r[15] = revision;
        r[20..24].copy_from_slice(&36u32.to_le_bytes());
        r[24..32].copy_from_slice(&xsdt.to_le_bytes());
        fix_checksum(&mut r[..20], 8);
        fix_checksum(&mut r, 32);
        r
    }

    fn lapic(id: u8, flags: u32) -> Vec<u8> {
        let mut e = vec![0, 8, id, id];
        e.extend(flags.to_le_bytes());
        e
    }

    fn x2apic(id: u32, flags: u32) -> Vec<u8> {
        let mut e = vec![9, 16, 0, 0];
        e.extend(id.to_le_bytes());
        e.extend(flags.to_le_bytes());
        e.extend(id.to_le_bytes());
        e
    }

    fn madt(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut body = 0xfee0_0000u32.to_le_bytes().to_vec();
        body.extend(1u32.to_le_bytes());
        for e in entries {
            body.extend(e);
        }
        sdt(*b"APIC", &body)
    }

    fn fadt(port: u32, extended: Option<(u8, u64)>) -> Vec<u8> {
        let mut body = vec![0u8; 276 - 36];
        body[76 - 36..80 - 36].copy_from_slice(&port.to_le_bytes());
        if let Some((space, address)) = extended {
            body[208 - 36] = space;
            body[212 - 36..220 - 36].copy_from_slice(&address.to_le_bytes());
        }
        sdt(*b"FACP", &body)
    }

    /// A machine's tables laid out from 0x1000: the root pointer, then
    /// the XSDT, then each listed table.
    fn machine(tables: &[Vec<u8>]) -> (Ram, u64) {
        let base = 0x1000u64;
        let mut addresses = Vec::new();
        let mut at = base + 36 + 36 + 8 * tables.len() as u64;
        for t in tables {
            addresses.push(at);
            at += t.len() as u64;
        }
        let list: Vec<u8> = addresses.iter().flat_map(|a| a.to_le_bytes()).collect();
        let mut bytes = rsdp(base + 36, 2);
        bytes.extend(sdt(*b"XSDT", &list));
        for t in tables {
            bytes.extend(t);
        }
        (Ram { base, bytes }, base)
    }

    #[test]
    fn the_root_pointer_leads_to_the_xsdt() {
        let (ram, rsdp_at) = machine(&[madt(&[])]);
        assert_eq!(xsdt_address(&ram, rsdp_at), Ok(rsdp_at + 36));
    }

    #[test]
    fn a_broken_root_pointer_is_refused() {
        let good = rsdp(0x2000, 2);
        let ram = |bytes: Vec<u8>| Ram { base: 0, bytes };
        let mut sig = good.clone();
        sig[0] = b'X';
        assert_eq!(xsdt_address(&ram(sig), 0), Err(Error::Signature));
        let mut first = good.clone();
        first[12] ^= 1;
        assert_eq!(
            xsdt_address(&ram(first), 0),
            Err(Error::Checksum),
            "the first 20 bytes"
        );
        let mut extended = good.clone();
        extended[28] ^= 1;
        assert_eq!(
            xsdt_address(&ram(extended), 0),
            Err(Error::Checksum),
            "all 36 bytes"
        );
        assert_eq!(xsdt_address(&ram(rsdp(0x2000, 0)), 0), Err(Error::TooOld));
        assert_eq!(
            xsdt_address(&ram(good[..30].to_vec()), 0),
            Err(Error::Truncated)
        );
        assert_eq!(
            xsdt_address(&ram(good.clone()), 100),
            Err(Error::Truncated),
            "nothing there"
        );
        assert_eq!(xsdt_address(&ram(good), 0), Ok(0x2000));
    }

    #[test]
    fn the_root_pointer_needs_acpi_2() {
        let ram = |bytes: Vec<u8>| Ram { base: 0, bytes };
        assert_eq!(xsdt_address(&ram(rsdp(0x2000, 1)), 0), Err(Error::TooOld));
        assert_eq!(xsdt_address(&ram(rsdp(0x2000, 3)), 0), Ok(0x2000));
    }

    #[test]
    fn a_short_length_still_checks_all_36_bytes() {
        let mut short = rsdp(0x2000, 2);
        short[20..24].copy_from_slice(&20u32.to_le_bytes());
        fix_checksum(&mut short[..20], 8);
        short[32] = short[32].wrapping_add(1);
        assert_eq!(
            xsdt_address(
                &Ram {
                    base: 0,
                    bytes: short
                },
                0
            ),
            Err(Error::Checksum)
        );
    }

    #[test]
    fn tables_are_found_by_signature() {
        let (ram, rsdp_at) = machine(&[fadt(0x608, None), madt(&[lapic(0, 1)])]);
        let xsdt = xsdt_address(&ram, rsdp_at).unwrap();
        let apic = find(&ram, xsdt, b"APIC").unwrap();
        assert_eq!(
            table(&ram, apic, b"APIC").map(|t| &t[..4]),
            Ok(&b"APIC"[..])
        );
        let facp = find(&ram, xsdt, b"FACP").unwrap();
        assert!(facp < apic, "in the order the XSDT lists them");
        assert_eq!(find(&ram, xsdt, b"HPET"), Err(Error::Missing(*b"HPET")));
    }

    #[test]
    fn broken_tables_are_refused() {
        let good = madt(&[lapic(0, 1)]);
        let ram = |bytes: Vec<u8>| Ram {
            base: 0x4000,
            bytes,
        };
        assert_eq!(
            table(&ram(good.clone()), 0x4000, b"FACP"),
            Err(Error::Signature)
        );
        let mut flipped = good.clone();
        flipped[40] ^= 0x10;
        assert_eq!(table(&ram(flipped), 0x4000, b"APIC"), Err(Error::Checksum));
        let mut short = good.clone();
        short[4..8].copy_from_slice(&35u32.to_le_bytes());
        assert_eq!(table(&ram(short), 0x4000, b"APIC"), Err(Error::Truncated));
        let mut long = good.clone();
        long[4..8].copy_from_slice(&4000u32.to_le_bytes());
        assert_eq!(table(&ram(long), 0x4000, b"APIC"), Err(Error::Truncated));
        assert_eq!(
            table(&ram(good.clone()), 0x4000, b"APIC").map(<[u8]>::len),
            Ok(good.len())
        );
    }

    #[test]
    fn a_listed_table_that_is_not_there_is_skipped() {
        let mut list = 0xdead_0000u64.to_le_bytes().to_vec();
        let apic_at = 0x1000 + 36 + 16;
        list.extend(u64::to_le_bytes(apic_at));
        let mut bytes = sdt(*b"XSDT", &list);
        bytes.extend(madt(&[]));
        let ram = Ram {
            base: 0x1000,
            bytes,
        };
        assert_eq!(find(&ram, 0x1000, b"APIC"), Ok(apic_at));
    }

    #[test]
    fn processors_are_counted_whatever_their_entry() {
        // x2APIC ids whose low bits are clear or set against their flags,
        // so reading the id where the flags are would count them wrong.
        let t = madt(&[
            lapic(0, 1),
            lapic(1, 1),
            lapic(2, 0),
            lapic(3, 2),
            x2apic(0x100, 1),
            x2apic(0x104, 2),
            x2apic(0x203, 0),
        ]);
        let found = processors(&t).unwrap();
        assert_eq!(found.listed, 7);
        assert_eq!(found.usable, 5, "enabled or online capable");
        assert_eq!(found.local_apic, 0xfee0_0000);
    }

    #[test]
    fn each_processor_comes_with_its_apic_id() {
        let t = madt(&[
            lapic(0, 1),
            lapic(7, 0),
            vec![1, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0],
            x2apic(0x1_0203, 2),
        ]);
        let mut seen = Vec::new();
        each_processor(&t, |p| seen.push(p)).unwrap();
        assert_eq!(
            seen,
            [
                Processor {
                    apic_id: 0,
                    usable: true
                },
                Processor {
                    apic_id: 7,
                    usable: false
                },
                Processor {
                    apic_id: 0x1_0203,
                    usable: true
                },
            ]
        );
        let mut short = lapic(3, 1);
        short.truncate(3);
        short[1] = 3;
        assert_eq!(
            each_processor(&madt(&[short]), |_| {}),
            Err(Error::Truncated),
            "no flags"
        );
        let mut short = x2apic(9, 1);
        short.truncate(6);
        short[1] = 6;
        assert_eq!(
            each_processor(&madt(&[short]), |_| {}),
            Err(Error::Truncated),
            "no whole id"
        );
    }

    #[test]
    fn other_entries_are_stepped_over() {
        // An I/O APIC (type 1, 12 bytes) and an override (type 2, 10 bytes).
        let io_apic = vec![1, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0];
        let over = vec![2, 10, 0, 0, 2, 0, 0, 0, 0, 0];
        let t = madt(&[io_apic, lapic(0, 1), over, lapic(1, 1)]);
        assert_eq!(processors(&t).map(|p| (p.listed, p.usable)), Ok((2, 2)));
    }

    #[test]
    fn a_64_bit_local_apic_address_wins() {
        let mut moved = vec![5, 12, 0, 0];
        moved.extend(0x1_fee0_0000u64.to_le_bytes());
        let t = madt(&[lapic(0, 1), moved]);
        assert_eq!(processors(&t).map(|p| p.local_apic), Ok(0x1_fee0_0000));
    }

    #[test]
    fn broken_entries_are_refused() {
        let mut zero = madt(&[lapic(0, 1)]);
        zero.extend([0, 0]);
        assert_eq!(
            processors(&zero),
            Err(Error::Truncated),
            "an entry of length 0 would never end"
        );
        let mut past = madt(&[]);
        past.extend([0, 8, 0, 0]);
        assert_eq!(
            processors(&past),
            Err(Error::Truncated),
            "an entry past the end"
        );
        let mut lonely = madt(&[]);
        lonely.push(0);
        assert_eq!(
            processors(&lonely),
            Err(Error::Truncated),
            "a type without a length"
        );
        assert_eq!(
            processors(&madt(&[])),
            Ok(Processors {
                usable: 0,
                listed: 0,
                local_apic: 0xfee0_0000
            })
        );
        assert_eq!(processors(&[0; 20]), Err(Error::Truncated));
    }

    #[test]
    fn the_timer_port_comes_from_the_extended_block_first() {
        assert_eq!(pm_timer_port(&fadt(0x608, Some((1, 0xb008)))), Some(0xb008));
        assert_eq!(
            pm_timer_port(&fadt(0x608, Some((1, 0)))),
            Some(0x608),
            "an empty extended block"
        );
        assert_eq!(
            pm_timer_port(&fadt(0x608, Some((0, 0xfed0_0000)))),
            Some(0x608),
            "memory, not I/O"
        );
        assert_eq!(pm_timer_port(&fadt(0x608, None)), Some(0x608));
        assert_eq!(pm_timer_port(&fadt(0, None)), None, "no timer");
        assert_eq!(
            pm_timer_port(&fadt(0x1_0000, None)),
            None,
            "not an I/O port"
        );
        assert_eq!(pm_timer_port(&fadt(0, Some((1, 0x1_0000)))), None);
        assert_eq!(
            pm_timer_port(&sdt(*b"FACP", &[0; 50])),
            None,
            "an old, short FADT"
        );
        assert_eq!(PM_TIMER_HZ, 3_579_545);
    }

    #[test]
    fn the_timer_is_24_bits_unless_the_flags_say_32() {
        let mut t = fadt(0x608, None);
        assert_eq!(pm_timer_bits(&t), 24);
        t[112..116].copy_from_slice(&(1u32 << 8).to_le_bytes());
        assert_eq!(pm_timer_bits(&t), 32);
        t[112..116].copy_from_slice(&(!(1u32 << 8)).to_le_bytes());
        assert_eq!(pm_timer_bits(&t), 24, "only bit 8 matters");
        assert_eq!(pm_timer_bits(&t[..100]), 24, "a short FADT");
    }

    #[test]
    fn checksums_add_up_to_zero() {
        assert!(checksum_ok(&[]));
        assert!(checksum_ok(&[0x80, 0x80]));
        assert!(!checksum_ok(&[1]));
        let t = sdt(*b"TEST", b"body");
        assert!(checksum_ok(&t));
    }

    fn hpet(space: u8, address: u64) -> Vec<u8> {
        let mut body = vec![0u8; 20];
        body[0..4].copy_from_slice(&0x8086_a201u32.to_le_bytes());
        body[4] = space;
        body[5] = 64;
        body[8..16].copy_from_slice(&address.to_le_bytes());
        sdt(*b"HPET", &body)
    }

    fn mcfg(entries: &[(u64, u16, u8, u8)]) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        for &(base, segment, first, last) in entries {
            body.extend_from_slice(&base.to_le_bytes());
            body.extend_from_slice(&segment.to_le_bytes());
            body.extend_from_slice(&[first, last, 0, 0, 0, 0]);
        }
        sdt(*b"MCFG", &body)
    }

    #[test]
    fn the_mcfg_lists_where_configuration_space_lies() {
        let one = mcfg(&[(0xb000_0000, 0, 0, 0xff)]);
        let regions: Vec<Ecam> = ecam_regions(&one).collect();
        assert_eq!(
            regions,
            [Ecam {
                base: 0xb000_0000,
                segment: 0,
                first_bus: 0,
                last_bus: 0xff
            }]
        );
        let two = mcfg(&[(0xe000_0000, 0, 0, 0x7f), (0x1_0000_0000, 1, 0x80, 0xff)]);
        let regions: Vec<Ecam> = ecam_regions(&two).collect();
        assert_eq!(regions.len(), 2);
        assert_eq!(
            (
                regions[1].base,
                regions[1].segment,
                regions[1].first_bus,
                regions[1].last_bus
            ),
            (0x1_0000_0000, 1, 0x80, 0xff)
        );
        assert_eq!(
            ecam_regions(&mcfg(&[])).count(),
            0,
            "a table without entries"
        );
    }

    #[test]
    fn mcfg_entries_end_where_the_table_does() {
        let mut t = mcfg(&[(0xb000_0000, 0, 0, 0xff), (0xc000_0000, 0, 0, 0xff)]);
        assert_eq!(
            ecam_regions(&t[..t.len() - 1]).count(),
            1,
            "a half entry is none"
        );
        t[4..8].copy_from_slice(&60u32.to_le_bytes());
        assert_eq!(ecam_regions(&t).count(), 1, "its length says one entry");
        t[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            ecam_regions(&t).count(),
            2,
            "a length past the bytes there are"
        );
        assert_eq!(ecam_regions(&t[..40]).count(), 0, "cut inside the header");
    }

    #[test]
    fn the_hpet_is_found_in_memory() {
        assert_eq!(hpet_address(&hpet(0, 0xfed0_0000)), Some(0xfed0_0000));
        assert_eq!(hpet_address(&hpet(0, 0x1_0000_0000)), Some(0x1_0000_0000));
        assert_eq!(hpet_address(&hpet(1, 0xfed0_0000)), None, "I/O space");
        assert_eq!(hpet_address(&hpet(0, 0)), None, "no address");
        let t = hpet(0, 0xfed0_0000);
        assert_eq!(hpet_address(&t[..51]), None, "cut short");
        assert_eq!(hpet_address(&t[..40]), None, "cut before the address");
        assert_eq!(
            hpet_address(&t[..52]),
            Some(0xfed0_0000),
            "the address is all it needs"
        );
    }

    #[test]
    fn tables_and_entries_at_their_shortest_are_read() {
        let bare = sdt(*b"SSDT", &[]);
        let ram = Ram {
            base: 0x1000,
            bytes: bare,
        };
        assert_eq!(
            table(&ram, 0x1000, b"SSDT").map(<[u8]>::len),
            Ok(36),
            "a table can be its header alone"
        );
        let t = madt(&[vec![0x7f, 2], lapic(0, 1)]);
        assert_eq!(
            processors(&t).map(|p| p.listed),
            Ok(1),
            "an entry of only its kind and length is skipped whole"
        );
    }
}
