//! Reads ELF64 executables for `x86_64` and places them in memory, the way
//! the loader places the kernel: every loadable segment copied to its spot,
//! the rest zeroed, and position-independent code relocated to where it
//! will run. Everything is read from the bytes with bounds checks, so a
//! broken file gives an error, never a panic.

#![cfg_attr(not(test), no_std)]
// Meridian only runs on 64-bit machines, and every offset is checked
// against a slice length before it becomes an index.
#![allow(clippy::cast_possible_truncation)]

const _: () = assert!(usize::BITS == 64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than its own headers say.
    Truncated,
    NotElf,
    Not64Bit,
    NotLittleEndian,
    NotX86_64,
    /// Neither an executable nor a position-independent executable.
    NotExecutable(u16),
    /// A program header lies outside the file or has an unexpected size.
    BadProgramHeaders,
    /// A segment claims more file bytes than it has memory, or its numbers
    /// overflow.
    BadSegment(usize),
    /// Loadable segments overlap or are not in ascending order.
    OverlappingSegments,
    NoLoadableSegments,
    /// The image given to `load` is not the size `span` reported.
    WrongImageSize,
    BadDynamicSection,
    UnsupportedRelocation(u32),
    RelocationOutsideImage(u64),
    /// A fixed-address executable cannot be moved to another base.
    CannotRelocate,
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PROGRAM_HEADER_SIZE: usize = 56;
const PAGE: u64 = 4096;

const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;
const R_X86_64_NONE: u32 = 0;
const R_X86_64_RELATIVE: u32 = 8;
const RELA_SIZE: u64 = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl Segment {
    #[must_use]
    pub fn end(&self) -> u64 {
        self.vaddr + self.mem_size
    }
}

pub struct Elf<'a> {
    data: &'a [u8],
    entry: u64,
    position_independent: bool,
    headers: &'a [u8],
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn page_down(v: u64) -> u64 {
    v & !(PAGE - 1)
}

impl<'a> Elf<'a> {
    /// Checks the file header and finds the program headers.
    ///
    /// # Errors
    ///
    /// Says what is wrong with the file: not ELF, not 64-bit little-endian
    /// `x86_64`, not executable, or headers outside the file.
    pub fn parse(data: &'a [u8]) -> Result<Elf<'a>, Error> {
        if data.len() < 64 {
            return Err(Error::Truncated);
        }
        if data[..4] != *b"\x7fELF" {
            return Err(Error::NotElf);
        }
        if data[4] != 2 {
            return Err(Error::Not64Bit);
        }
        if data[5] != 1 {
            return Err(Error::NotLittleEndian);
        }
        let kind = u16_at(data, 16).ok_or(Error::Truncated)?;
        if u16_at(data, 18) != Some(EM_X86_64) {
            return Err(Error::NotX86_64);
        }
        if kind != ET_EXEC && kind != ET_DYN {
            return Err(Error::NotExecutable(kind));
        }
        let entry = u64_at(data, 24).ok_or(Error::Truncated)?;
        let phoff = u64_at(data, 32).ok_or(Error::Truncated)?;
        let entsize = u16_at(data, 54).ok_or(Error::Truncated)?;
        let count = u16_at(data, 56).ok_or(Error::Truncated)?;
        if usize::from(entsize) != PROGRAM_HEADER_SIZE {
            return Err(Error::BadProgramHeaders);
        }
        let start = usize::try_from(phoff).map_err(|_| Error::BadProgramHeaders)?;
        let len = usize::from(count) * PROGRAM_HEADER_SIZE;
        let headers = start
            .checked_add(len)
            .and_then(|end| data.get(start..end))
            .ok_or(Error::BadProgramHeaders)?;
        Ok(Elf {
            data,
            entry,
            position_independent: kind == ET_DYN,
            headers,
        })
    }

    /// The entry point as linked, before any relocation.
    #[must_use]
    pub fn entry(&self) -> u64 {
        self.entry
    }

    #[must_use]
    pub fn position_independent(&self) -> bool {
        self.position_independent
    }

    fn program_headers(&self) -> impl Iterator<Item = (u32, &'a [u8])> + '_ {
        self.headers
            .as_chunks::<PROGRAM_HEADER_SIZE>()
            .0
            .iter()
            .map(|h| (u32::from_le_bytes([h[0], h[1], h[2], h[3]]), h.as_slice()))
    }

    /// The loadable segments, checked against the file.
    ///
    /// # Errors
    ///
    /// Names the first segment whose numbers do not make sense.
    pub fn segments(&self) -> Result<SegmentList, Error> {
        let mut list = SegmentList::default();
        let mut previous_end = 0u64;
        for (i, (kind, h)) in self.program_headers().enumerate() {
            if kind != PT_LOAD {
                continue;
            }
            let read_u64 = |at| u64_at(h, at).ok_or(Error::BadSegment(i));
            let flags = u32_at(h, 4).ok_or(Error::BadSegment(i))?;
            let segment = Segment {
                file_offset: read_u64(8)?,
                vaddr: read_u64(16)?,
                file_size: read_u64(32)?,
                mem_size: read_u64(40)?,
                execute: flags & 1 != 0,
                write: flags & 2 != 0,
                read: flags & 4 != 0,
            };
            let file_end = segment.file_offset.checked_add(segment.file_size);
            let mem_end = segment.vaddr.checked_add(segment.mem_size);
            if segment.file_size > segment.mem_size
                || mem_end.is_none()
                || file_end.is_none_or(|end| end > self.data.len() as u64)
            {
                return Err(Error::BadSegment(i));
            }
            if segment.vaddr < previous_end {
                return Err(Error::OverlappingSegments);
            }
            previous_end = segment.end();
            list.push(segment).ok_or(Error::BadProgramHeaders)?;
        }
        if list.len == 0 {
            return Err(Error::NoLoadableSegments);
        }
        Ok(list)
    }

    /// The first page-aligned address the image covers, and its size in
    /// bytes rounded up to whole pages.
    ///
    /// # Errors
    ///
    /// Fails when the segments do.
    pub fn span(&self) -> Result<(u64, u64), Error> {
        let segments = self.segments()?;
        let first = page_down(segments.as_slice()[0].vaddr);
        let last = segments
            .as_slice()
            .iter()
            .map(Segment::end)
            .max()
            .unwrap_or(first);
        let size = (last - first).div_ceil(PAGE) * PAGE;
        Ok((first, size))
    }

    /// Places the image into `image`, which stands for the memory from
    /// `span().0` on and must be exactly `span().1` bytes, as it will run
    /// at `base` instead of `span().0`. Relocations are applied for the
    /// difference.
    ///
    /// # Errors
    ///
    /// Fails on a wrong image size, a fixed-address file moved elsewhere,
    /// a broken dynamic section, or a relocation Meridian does not handle.
    pub fn load(&self, image: &mut [u8], base: u64) -> Result<(), Error> {
        let (first, size) = self.span()?;
        if image.len() as u64 != size {
            return Err(Error::WrongImageSize);
        }
        if base != first && !self.position_independent {
            return Err(Error::CannotRelocate);
        }
        image.fill(0);
        for s in self.segments()?.as_slice() {
            let at = (s.vaddr - first) as usize;
            let from = s.file_offset as usize;
            image[at..at + s.file_size as usize]
                .copy_from_slice(&self.data[from..from + s.file_size as usize]);
        }
        self.relocate(image, first, base.wrapping_sub(first))
    }

    fn relocate(&self, image: &mut [u8], first: u64, bias: u64) -> Result<(), Error> {
        let Some((_, dynamic)) = self.program_headers().find(|(kind, _)| *kind == PT_DYNAMIC)
        else {
            return Ok(());
        };
        let offset = u64_at(dynamic, 8).ok_or(Error::BadDynamicSection)?;
        let size = u64_at(dynamic, 32).ok_or(Error::BadDynamicSection)?;
        let entries = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(size).ok())
            .and_then(|(o, s)| self.data.get(o..o.checked_add(s)?))
            .ok_or(Error::BadDynamicSection)?;
        let (mut rela, mut rela_size, mut rela_entry) = (None, 0, RELA_SIZE);
        for entry in entries.as_chunks::<16>().0 {
            let halves = entry.as_chunks::<8>().0;
            let tag = i64::from_le_bytes(halves[0]);
            let value = u64::from_le_bytes(halves[1]);
            match tag {
                DT_NULL => break,
                DT_RELA => rela = Some(value),
                DT_RELASZ => rela_size = value,
                DT_RELAENT => rela_entry = value,
                _ => {}
            }
        }
        let Some(rela) = rela else {
            return Ok(());
        };
        if rela_entry != RELA_SIZE {
            return Err(Error::BadDynamicSection);
        }
        let table = rela
            .checked_sub(first)
            .and_then(|start| Some((start, start.checked_add(rela_size)?)))
            .filter(|(_, end)| *end <= image.len() as u64)
            .ok_or(Error::BadDynamicSection)?;
        for at in (table.0..table.1).step_by(RELA_SIZE as usize) {
            let at = at as usize;
            let where_ = u64_at(image, at).ok_or(Error::BadDynamicSection)?;
            let info = u64_at(image, at + 8).ok_or(Error::BadDynamicSection)?;
            let addend = u64_at(image, at + 16).ok_or(Error::BadDynamicSection)?;
            match (info & 0xffff_ffff) as u32 {
                R_X86_64_NONE => {}
                R_X86_64_RELATIVE => {
                    let target = where_
                        .checked_sub(first)
                        .map(|t| t as usize)
                        .and_then(|t| image.get_mut(t..t.checked_add(8)?))
                        .ok_or(Error::RelocationOutsideImage(where_))?;
                    target.copy_from_slice(&addend.wrapping_add(bias).to_le_bytes());
                }
                other => return Err(Error::UnsupportedRelocation(other)),
            }
        }
        Ok(())
    }
}

/// Up to [`SegmentList::CAPACITY`] segments without allocating. Executables
/// Meridian builds have three or four.
#[derive(Clone, Copy, Debug, Default)]
pub struct SegmentList {
    items: [Segment; SegmentList::CAPACITY],
    len: usize,
}

impl SegmentList {
    pub const CAPACITY: usize = 16;

    fn push(&mut self, s: Segment) -> Option<()> {
        *self.items.get_mut(self.len)? = s;
        self.len += 1;
        Some(())
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Segment] {
        &self.items[..self.len]
    }

    /// What the page holding address `at`, as linked, may be used for:
    /// whatever any segment on it allows, since a page has one set of
    /// rights. None where no segment lies.
    #[must_use]
    pub fn access(&self, at: u64) -> Option<Access> {
        let page = page_down(at);
        self.as_slice()
            .iter()
            .filter(|s| s.mem_size > 0 && s.vaddr < page.saturating_add(PAGE) && s.end() > page)
            .map(|s| Access {
                write: s.write,
                execute: s.execute,
            })
            .reduce(|a, b| Access {
                write: a.write || b.write,
                execute: a.execute || b.execute,
            })
    }
}

/// What a page of an image may be used for besides being read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub write: bool,
    pub execute: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Seg {
        kind: u32,
        flags: u32,
        vaddr: u64,
        file: Vec<u8>,
        mem_size: u64,
    }

    fn load_seg(flags: u32, vaddr: u64, file: &[u8], mem_size: u64) -> Seg {
        Seg {
            kind: PT_LOAD,
            flags,
            vaddr,
            file: file.to_vec(),
            mem_size,
        }
    }

    /// Writes an ELF64 file: header, program headers, then each segment's
    /// bytes in order. Offsets in the headers point at those bytes.
    fn build(kind: u16, entry: u64, segs: &[Seg]) -> Vec<u8> {
        let mut out = vec![0u8; 64];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 2;
        out[5] = 1;
        out[6] = 1;
        out[16..18].copy_from_slice(&kind.to_le_bytes());
        out[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        out[24..32].copy_from_slice(&entry.to_le_bytes());
        out[32..40].copy_from_slice(&64u64.to_le_bytes());
        out[52..54].copy_from_slice(&64u16.to_le_bytes());
        out[54..56].copy_from_slice(&56u16.to_le_bytes());
        out[56..58].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        let mut offset = 64 + 56 * segs.len() as u64;
        let mut headers = Vec::new();
        for s in segs {
            let mut h = [0u8; 56];
            h[0..4].copy_from_slice(&s.kind.to_le_bytes());
            h[4..8].copy_from_slice(&s.flags.to_le_bytes());
            h[8..16].copy_from_slice(&offset.to_le_bytes());
            h[16..24].copy_from_slice(&s.vaddr.to_le_bytes());
            h[32..40].copy_from_slice(&(s.file.len() as u64).to_le_bytes());
            h[40..48].copy_from_slice(&s.mem_size.to_le_bytes());
            headers.extend_from_slice(&h);
            offset += s.file.len() as u64;
        }
        out.extend(headers);
        for s in segs {
            out.extend_from_slice(&s.file);
        }
        out
    }

    fn simple() -> Vec<u8> {
        build(
            ET_DYN,
            0x1000,
            &[
                load_seg(5, 0x1000, b"code", 4),
                load_seg(6, 0x3000, b"data", 0x2000),
            ],
        )
    }

    #[test]
    fn reads_the_header() {
        let file = simple();
        let elf = Elf::parse(&file).unwrap();
        assert_eq!(elf.entry(), 0x1000);
        assert!(elf.position_independent());
        let fixed = build(ET_EXEC, 7, &[load_seg(5, 0x1000, b"x", 1)]);
        assert!(!Elf::parse(&fixed).unwrap().position_independent());
    }

    #[test]
    fn refuses_what_is_not_an_x86_64_executable() {
        let good = simple();
        let mut cases: Vec<(Vec<u8>, Error)> = vec![(good[..63].to_vec(), Error::Truncated)];
        let mut set = |at: usize, bytes: &[u8], err: Error| {
            let mut f = good.clone();
            f[at..at + bytes.len()].copy_from_slice(bytes);
            cases.push((f, err));
        };
        set(0, b"\x7fELG", Error::NotElf);
        set(4, &[1], Error::Not64Bit);
        set(4, &[0], Error::Not64Bit);
        set(5, &[2], Error::NotLittleEndian);
        set(5, &[0], Error::NotLittleEndian);
        set(18, &3u16.to_le_bytes(), Error::NotX86_64);
        set(18, &0u16.to_le_bytes(), Error::NotX86_64);
        set(16, &1u16.to_le_bytes(), Error::NotExecutable(1));
        set(16, &4u16.to_le_bytes(), Error::NotExecutable(4));
        set(54, &64u16.to_le_bytes(), Error::BadProgramHeaders);
        set(54, &32u16.to_le_bytes(), Error::BadProgramHeaders);
        set(
            32,
            &0xffff_ffff_ffff_fff0u64.to_le_bytes(),
            Error::BadProgramHeaders,
        );
        set(56, &60u16.to_le_bytes(), Error::BadProgramHeaders);
        for (file, want) in cases {
            assert_eq!(Elf::parse(&file).err(), Some(want));
        }
    }

    #[test]
    fn segments_and_their_permissions() {
        let file = build(
            ET_DYN,
            0,
            &[
                load_seg(5, 0x1000, b"text", 4),
                Seg {
                    kind: 4,
                    flags: 4,
                    vaddr: 0,
                    file: b"note".to_vec(),
                    mem_size: 4,
                },
                load_seg(4, 0x2000, b"ro", 2),
                load_seg(6, 0x3000, b"rw", 0x100),
            ],
        );
        let list = Elf::parse(&file).unwrap().segments().unwrap();
        let s = list.as_slice();
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].read, s[0].write, s[0].execute), (true, false, true));
        assert_eq!((s[1].read, s[1].write, s[1].execute), (true, false, false));
        assert_eq!((s[2].read, s[2].write, s[2].execute), (true, true, false));
        assert_eq!(
            (s[2].vaddr, s[2].file_size, s[2].mem_size),
            (0x3000, 2, 0x100)
        );
        assert_eq!(s[2].end(), 0x3100);
        assert_eq!(s[0].file_offset, 64 + 56 * 4);
    }

    #[test]
    fn each_page_allows_what_its_segments_do() {
        let file = build(
            ET_DYN,
            0x2000,
            &[
                load_seg(4, 0, b"ro", 0x1200),
                load_seg(5, 0x2000, b"text", 0x100),
                load_seg(6, 0x2f00, b"data", 0x200),
            ],
        );
        let segments = Elf::parse(&file).unwrap().segments().unwrap();
        let read = Access::default();
        let write = Access {
            write: true,
            execute: false,
        };
        assert_eq!(segments.access(0), Some(read));
        assert_eq!(
            segments.access(0x1fff),
            Some(read),
            "the second page of the first"
        );
        assert_eq!(
            segments.access(0x2000),
            Some(Access {
                write: true,
                execute: true
            }),
            "code and data share a page"
        );
        assert_eq!(segments.access(0x2abc), segments.access(0x2000));
        assert_eq!(segments.access(0x3000), Some(write), "data alone");
        assert_eq!(segments.access(0x30ff), Some(write));
        assert_eq!(segments.access(0x4000), None, "past the end");
        assert_eq!(segments.access(u64::MAX), None);
    }

    #[test]
    fn a_page_between_segments_or_under_an_empty_one_has_none() {
        let file = build(
            ET_DYN,
            0,
            &[
                load_seg(5, 0, b"text", 0x1000),
                load_seg(6, 0x3000, b"", 0),
                load_seg(6, 0x3800, b"", 0),
                load_seg(6, 0x5000, b"", 0x10),
            ],
        );
        let segments = Elf::parse(&file).unwrap().segments().unwrap();
        assert_eq!(
            segments.access(0),
            Some(Access {
                write: false,
                execute: true
            })
        );
        assert_eq!(segments.access(0x1000), None, "the text ends with its page");
        assert_eq!(segments.access(0x2000), None);
        assert_eq!(
            segments.access(0x3000),
            None,
            "an empty segment covers nothing"
        );
        assert_eq!(segments.access(0x3800), None, "not even inside a page");
        assert_eq!(segments.access(0x4fff), None);
        assert!(
            segments
                .access(0x5000)
                .is_some_and(|a| a.write && !a.execute)
        );
    }

    #[test]
    fn broken_segments_are_named() {
        let bigger_file = build(ET_DYN, 0, &[load_seg(4, 0x1000, b"abc", 2)]);
        let past_end = {
            let mut f = build(ET_DYN, 0, &[load_seg(4, 0x1000, b"abcd", 4)]);
            f.truncate(f.len() - 1);
            f
        };
        let overflow = build(ET_DYN, 0, &[load_seg(4, u64::MAX - 1, b"ab", 2)]);
        let second_bad = build(
            ET_DYN,
            0,
            &[
                load_seg(4, 0x1000, b"ok", 2),
                load_seg(4, 0x2000, b"abcd", 1),
            ],
        );
        for (file, want) in [
            (bigger_file, Error::BadSegment(0)),
            (past_end, Error::BadSegment(0)),
            (overflow, Error::BadSegment(0)),
            (second_bad, Error::BadSegment(1)),
        ] {
            assert_eq!(Elf::parse(&file).unwrap().segments().err(), Some(want));
        }
    }

    #[test]
    fn segments_must_not_overlap_or_go_backwards() {
        let overlap = build(
            ET_DYN,
            0,
            &[
                load_seg(4, 0x1000, b"", 0x1001),
                load_seg(4, 0x2000, b"", 1),
            ],
        );
        let backwards = build(
            ET_DYN,
            0,
            &[load_seg(4, 0x3000, b"", 1), load_seg(4, 0x1000, b"", 1)],
        );
        let touching = build(
            ET_DYN,
            0,
            &[
                load_seg(4, 0x1000, b"", 0x1000),
                load_seg(4, 0x2000, b"", 1),
            ],
        );
        let elf = |f: &[u8]| {
            Elf::parse(f)
                .unwrap()
                .segments()
                .map(|l| l.as_slice().len())
        };
        assert_eq!(elf(&overlap), Err(Error::OverlappingSegments));
        assert_eq!(elf(&backwards), Err(Error::OverlappingSegments));
        assert_eq!(elf(&touching), Ok(2));
    }

    #[test]
    fn a_file_without_loadable_segments() {
        let file = build(
            ET_DYN,
            0,
            &[Seg {
                kind: 4,
                flags: 4,
                vaddr: 0,
                file: vec![],
                mem_size: 0,
            }],
        );
        let elf = Elf::parse(&file).unwrap();
        assert_eq!(elf.segments().err(), Some(Error::NoLoadableSegments));
        assert_eq!(elf.span().err(), Some(Error::NoLoadableSegments));
    }

    #[test]
    fn more_segments_than_the_list_holds() {
        let segs: Vec<Seg> = (0..17)
            .map(|i| load_seg(4, 0x1000 * (i + 1), b"", 1))
            .collect();
        let file = build(ET_DYN, 0, &segs);
        assert_eq!(
            Elf::parse(&file).unwrap().segments().err(),
            Some(Error::BadProgramHeaders)
        );
        let fits: Vec<Seg> = (0..16)
            .map(|i| load_seg(4, 0x1000 * (i + 1), b"", 1))
            .collect();
        let file = build(ET_DYN, 0, &fits);
        assert_eq!(
            Elf::parse(&file)
                .unwrap()
                .segments()
                .unwrap()
                .as_slice()
                .len(),
            16
        );
    }

    #[test]
    fn span_covers_whole_pages() {
        let file = build(
            ET_DYN,
            0,
            &[load_seg(5, 0x1234, b"a", 1), load_seg(6, 0x5000, b"", 0x10)],
        );
        assert_eq!(Elf::parse(&file).unwrap().span(), Ok((0x1000, 0x5000)));
        let exact = build(ET_DYN, 0, &[load_seg(5, 0x2000, b"", 0x1000)]);
        assert_eq!(Elf::parse(&exact).unwrap().span(), Ok((0x2000, 0x1000)));
    }

    #[test]
    fn load_copies_and_zeroes() {
        let file = simple();
        let elf = Elf::parse(&file).unwrap();
        let (first, size) = elf.span().unwrap();
        assert_eq!((first, size), (0x1000, 0x4000));
        let mut image = vec![0xaa; size as usize];
        elf.load(&mut image, first).unwrap();
        assert_eq!(&image[..4], b"code");
        assert!(image[4..0x2000].iter().all(|&b| b == 0), "gap is zeroed");
        assert_eq!(&image[0x2000..0x2004], b"data");
        assert!(image[0x2004..].iter().all(|&b| b == 0), "bss is zeroed");
    }

    #[test]
    fn load_checks_size_and_base() {
        let elf_file = simple();
        let elf = Elf::parse(&elf_file).unwrap();
        let mut small = vec![0; 0x3000];
        assert_eq!(elf.load(&mut small, 0x1000), Err(Error::WrongImageSize));
        let fixed = build(ET_EXEC, 0, &[load_seg(5, 0x1000, b"x", 1)]);
        let fixed = Elf::parse(&fixed).unwrap();
        let mut image = vec![0; 0x1000];
        assert_eq!(fixed.load(&mut image, 0x8000), Err(Error::CannotRelocate));
        assert_eq!(fixed.load(&mut image, 0x1000), Ok(()));
        assert_eq!(image[0], b'x');
    }

    fn dynamic(entries: &[(i64, u64)]) -> Vec<u8> {
        let mut d = Vec::new();
        for (tag, value) in entries {
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&value.to_le_bytes());
        }
        d
    }

    fn rela(entries: &[(u64, u32, u64)]) -> Vec<u8> {
        let mut r = Vec::new();
        for (offset, kind, addend) in entries {
            r.extend_from_slice(&offset.to_le_bytes());
            r.extend_from_slice(&u64::from(*kind).to_le_bytes());
            r.extend_from_slice(&addend.to_le_bytes());
        }
        r
    }

    /// A position-independent file with its relocation table at 0x2000 and
    /// a data page at 0x3000 for the relocations to patch.
    fn relocatable(relocs: &[(u64, u32, u64)], dyn_entries: &[(i64, u64)]) -> Vec<u8> {
        let table = rela(relocs);
        build(
            ET_DYN,
            0x1000,
            &[
                load_seg(5, 0x1000, b"code", 4),
                load_seg(4, 0x2000, &table, table.len() as u64),
                load_seg(6, 0x3000, &[0xee; 32], 0x1000),
                Seg {
                    kind: PT_DYNAMIC,
                    flags: 4,
                    vaddr: 0,
                    file: dynamic(dyn_entries),
                    mem_size: 0,
                },
            ],
        )
    }

    fn standard(relocs: &[(u64, u32, u64)]) -> Vec<u8> {
        relocatable(
            relocs,
            &[
                (DT_RELA, 0x2000),
                (DT_RELASZ, 24 * relocs.len() as u64),
                (DT_RELAENT, 24),
                (DT_NULL, 0),
            ],
        )
    }

    fn loaded(file: &[u8], base: u64) -> Result<Vec<u8>, Error> {
        let elf = Elf::parse(file)?;
        let (_, size) = elf.span()?;
        let mut image = vec![0u8; size as usize];
        elf.load(&mut image, base)?;
        Ok(image)
    }

    fn word(image: &[u8], vaddr: u64) -> u64 {
        u64_at(image, (vaddr - 0x1000) as usize).unwrap()
    }

    #[test]
    fn relative_relocations_at_the_linked_base() {
        let file = standard(&[(0x3000, 8, 0x1000), (0x3008, 8, 0x3010)]);
        let image = loaded(&file, 0x1000).unwrap();
        assert_eq!(word(&image, 0x3000), 0x1000);
        assert_eq!(word(&image, 0x3008), 0x3010);
        assert_eq!(word(&image, 0x3010), 0xeeee_eeee_eeee_eeee);
    }

    #[test]
    fn relative_relocations_move_with_the_base() {
        let file = standard(&[(0x3000, 8, 0x1000), (0x3018, 8, 0x1234)]);
        let base = 0xffff_ffff_8000_0000;
        let image = loaded(&file, base).unwrap();
        assert_eq!(word(&image, 0x3000), base);
        assert_eq!(word(&image, 0x3018), base + 0x234);
    }

    #[test]
    fn none_relocations_are_skipped_and_others_refused() {
        let file = standard(&[(0x3000, 0, 5)]);
        assert_eq!(
            word(&loaded(&file, 0x1000).unwrap(), 0x3000),
            0xeeee_eeee_eeee_eeee
        );
        let file = standard(&[(0x3000, 1, 5)]);
        assert_eq!(loaded(&file, 0x1000), Err(Error::UnsupportedRelocation(1)));
        let file = standard(&[(0x3000, 8 | 0x10, 5)]);
        assert_eq!(
            loaded(&file, 0x1000),
            Err(Error::UnsupportedRelocation(0x18))
        );
    }

    #[test]
    fn the_symbol_half_of_info_does_not_change_the_type() {
        let file = relocatable(
            &[(0x3000, 8, 0x42)],
            &[(DT_RELA, 0x2000), (DT_RELASZ, 24), (DT_NULL, 0)],
        );
        let mut patched = file.clone();
        let table_at = patched
            .windows(8)
            .position(|w| w == 0x3000u64.to_le_bytes())
            .unwrap();
        patched[table_at + 12..table_at + 16].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(word(&loaded(&patched, 0x1000).unwrap(), 0x3000), 0x42);
    }

    #[test]
    fn relocations_outside_the_image_are_refused() {
        let file = standard(&[(0x9000, 8, 1)]);
        assert_eq!(
            loaded(&file, 0x1000),
            Err(Error::RelocationOutsideImage(0x9000))
        );
        let file = standard(&[(0x500, 8, 1)]);
        assert_eq!(
            loaded(&file, 0x1000),
            Err(Error::RelocationOutsideImage(0x500))
        );
        let file = standard(&[(0x4ffc, 8, 1)]);
        assert_eq!(
            loaded(&file, 0x1000),
            Err(Error::RelocationOutsideImage(0x4ffc))
        );
    }

    #[test]
    fn a_table_ending_exactly_at_the_image_end_is_read() {
        let table = rela(&[(0x1008, 8, 0x77)]);
        let file = build(
            ET_DYN,
            0x1000,
            &[
                load_seg(7, 0x1000, &[0xee; 16], 16),
                load_seg(4, 0x2fe8, &table, 24),
                Seg {
                    kind: PT_DYNAMIC,
                    flags: 4,
                    vaddr: 0,
                    file: dynamic(&[(DT_RELA, 0x2fe8), (DT_RELASZ, 24), (DT_NULL, 0)]),
                    mem_size: 0,
                },
            ],
        );
        assert_eq!(Elf::parse(&file).unwrap().span(), Ok((0x1000, 0x2000)));
        assert_eq!(word(&loaded(&file, 0x1000).unwrap(), 0x1008), 0x77);
    }

    #[test]
    fn relocation_types_above_one_byte_are_not_mistaken() {
        let file = standard(&[(0x3000, 0x108, 5)]);
        assert_eq!(
            loaded(&file, 0x1000),
            Err(Error::UnsupportedRelocation(0x108))
        );
    }

    #[test]
    fn broken_dynamic_sections() {
        let wrong_entry = relocatable(
            &[(0x3000, 8, 1)],
            &[
                (DT_RELA, 0x2000),
                (DT_RELASZ, 24),
                (DT_RELAENT, 16),
                (DT_NULL, 0),
            ],
        );
        assert_eq!(loaded(&wrong_entry, 0x1000), Err(Error::BadDynamicSection));
        let table_outside = relocatable(
            &[(0x3000, 8, 1)],
            &[(DT_RELA, 0x2000), (DT_RELASZ, 0x10_0000), (DT_NULL, 0)],
        );
        assert_eq!(
            loaded(&table_outside, 0x1000),
            Err(Error::BadDynamicSection)
        );
        let table_below = relocatable(
            &[(0x3000, 8, 1)],
            &[(DT_RELA, 0x10), (DT_RELASZ, 24), (DT_NULL, 0)],
        );
        assert_eq!(loaded(&table_below, 0x1000), Err(Error::BadDynamicSection));
        let mut cut = standard(&[(0x3000, 8, 1)]);
        let len = cut.len();
        cut.truncate(len - 8);
        assert!(Elf::parse(&cut).is_ok());
        assert_eq!(loaded(&cut, 0x1000), Err(Error::BadDynamicSection));
    }

    #[test]
    fn without_rela_nothing_is_relocated() {
        let file = relocatable(&[(0x3000, 8, 1)], &[(DT_RELASZ, 24), (DT_NULL, 0)]);
        assert_eq!(
            word(&loaded(&file, 0x1000).unwrap(), 0x3000),
            0xeeee_eeee_eeee_eeee
        );
    }

    #[test]
    fn entries_after_the_end_of_the_dynamic_section_are_ignored() {
        let file = relocatable(
            &[(0x3000, 8, 1)],
            &[(DT_NULL, 0), (DT_RELA, 0x2000), (DT_RELASZ, 24)],
        );
        assert_eq!(
            word(&loaded(&file, 0x1000).unwrap(), 0x3000),
            0xeeee_eeee_eeee_eeee
        );
    }

    // The test binary itself is a real position-independent executable. It
    // is linked against libc, so loading it must stop at the first
    // relocation that needs a symbol: Meridian only handles RELATIVE ones.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_executable_from_the_host() {
        let file = std::fs::read("/proc/self/exe").unwrap();
        let elf = Elf::parse(&file).unwrap();
        assert!(elf.position_independent());
        let segments = elf.segments().unwrap();
        assert!(segments.as_slice().len() >= 2);
        assert!(segments.as_slice().iter().any(|s| s.execute && !s.write));
        let (first, size) = elf.span().unwrap();
        assert_eq!(first % PAGE, 0);
        let mut image = vec![0u8; size as usize];
        match elf.load(&mut image, first) {
            Err(Error::UnsupportedRelocation(kind)) => {
                assert!(kind == 6 || kind == 7 || kind == 1, "{kind}");
            }
            other => panic!("expected an unsupported relocation, got {other:?}"),
        }
    }

    #[test]
    fn a_header_alone_is_an_elf_file_without_segments() {
        let bare = build(ET_DYN, 0x1000, &[]);
        assert_eq!(bare.len(), 64);
        let elf = Elf::parse(&bare).unwrap();
        assert_eq!(elf.segments().err(), Some(Error::NoLoadableSegments));
    }

    #[test]
    fn each_permission_comes_from_its_own_bit() {
        let file = build(
            ET_DYN,
            0x1000,
            &[
                load_seg(5, 0x1000, b"code", 4),
                load_seg(6, 0x3000, b"data", 4),
                load_seg(4, 0x5000, b"rodata", 6),
                load_seg(1, 0x7000, b"x", 1),
            ],
        );
        let elf = Elf::parse(&file).unwrap();
        let list = elf.segments().unwrap();
        let rights: Vec<(bool, bool, bool)> = list
            .as_slice()
            .iter()
            .map(|s| (s.read, s.write, s.execute))
            .collect();
        assert_eq!(
            rights,
            [
                (true, false, true),
                (true, true, false),
                (true, false, false),
                (false, false, true)
            ]
        );
    }
}
