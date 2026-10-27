//! Programs from files, the parts that need no processor: which of the
//! files the loader read can be trusted to be where it says, where the
//! pages of a program's ELF file go in its address space, and what each of
//! them may do.

use bootinfo::{BootInfo, HHDM_BASE, MemoryRegion, Module, Modules, RegionKind};
use elf::{Note, Segment};
use paging::Flags;

const PAGE: u64 = 4096;

/// Where a program's image starts in its address space.
pub const CODE_AT: u64 = 0x40_0000;

/// The top of a program's stack, and how many pages it has below.
pub const STACK_TOP: u64 = 0x80_0000;
pub const STACK_PAGES: u64 = 16;

/// The most an image may take: what lies between [`CODE_AT`] and the page
/// under the stack, which stays unmapped so a stack that runs over faults.
pub const IMAGE_MAX: u64 = STACK_TOP - (STACK_PAGES + 1) * PAGE - CODE_AT;

/// Where the memory a program asks for lies: from [`HEAP_START`] up to
/// [`HEAP_END`], 64 GiB of room. What was given back is never given
/// again, so a stale pointer faults instead of reaching new data.
pub const HEAP_START: u64 = 0x10_0000_0000;
pub const HEAP_END: u64 = 0x20_0000_0000;

/// The most pages one call gives, 64 MiB.
pub const MAP_MAX: u64 = 16384;

/// How much of a program's heap region was given out so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heap {
    next: u64,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    #[must_use]
    pub const fn new() -> Heap {
        Heap { next: HEAP_START }
    }

    /// Where `pages` more pages go, now given out.
    ///
    /// # Errors
    ///
    /// [`abi::Error::BadSize`] for 0 pages or more than [`MAP_MAX`], and
    /// [`abi::Error::NoRoom`] once the region is used up.
    pub fn take(&mut self, pages: u64) -> Result<u64, abi::Error> {
        if pages == 0 || pages > MAP_MAX {
            return Err(abi::Error::BadSize);
        }
        let at = self.next;
        let end = at + pages * PAGE;
        if end > HEAP_END {
            return Err(abi::Error::NoRoom);
        }
        self.next = end;
        Ok(at)
    }

    /// Whether the `pages` pages from `at` lie in what was given out.
    #[must_use]
    pub fn holds(&self, at: u64, pages: u64) -> bool {
        let end = pages
            .checked_mul(PAGE)
            .and_then(|bytes| at.checked_add(bytes));
        at.is_multiple_of(PAGE)
            && pages > 0
            && at >= HEAP_START
            && end.is_some_and(|e| e <= self.next)
    }
}

/// Why a program does not get to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Its image takes this many bytes, more than [`IMAGE_MAX`].
    TooBig(u64),
    /// The page at this address, as linked, would be writable and
    /// executable at once.
    WritableCode(u64),
    /// It would start at this address, as linked, on no executable page.
    BadEntry(u64),
    /// Its file is an ELF file, but carries no Meridian note saying it is
    /// a `.mexec` file.
    NotMeridian,
    /// Its file is a `.mexec` file of this version, which this kernel does
    /// not know.
    Version(u32),
}

/// What a program's file says about it in its Meridian notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata<'a> {
    /// Its name, empty when it gives none.
    pub name: &'a [u8],
}

/// Reads the Meridian notes among `notes`.
///
/// # Errors
///
/// When none says the file is a `.mexec` file, or one says it is of a
/// version this kernel does not know.
pub fn metadata<'a>(notes: impl Iterator<Item = Note<'a>>) -> Result<Metadata<'a>, Refusal> {
    let mut version = None;
    let mut name: &[u8] = &[];
    for note in notes.filter(|n| n.owner == abi::NOTE_OWNER) {
        match note.kind {
            abi::NOTE_VERSION => {
                version = Some(note.desc.try_into().map_or(0, u32::from_le_bytes));
            }
            abi::NOTE_NAME => name = note.desc,
            _ => {}
        }
    }
    match version {
        Some(abi::MEXEC_VERSION) => Ok(Metadata { name }),
        Some(other) => Err(Refusal::Version(other)),
        None => Err(Refusal::NotMeridian),
    }
}

/// Where the list of the files the loader read lies, physically, and how
/// many it holds: none when it is empty, longer than [`Modules::MAX`], or
/// not in the kernel's own memory, where the loader puts it.
#[must_use]
pub fn module_list(info: &BootInfo, regions: &[MemoryRegion]) -> Option<(u64, usize)> {
    let count = usize::try_from(info.modules.count)
        .ok()
        .filter(|&c| c > 0 && c <= Modules::MAX)?;
    let at = info.modules.list.checked_sub(HHDM_BASE)?;
    let bytes = (count * size_of::<Module>()) as u64;
    crate::covered(regions, RegionKind::KERNEL, at, bytes).then_some((at, count))
}

/// The name of the file the loader read that the file at `path` is: one
/// in `/System/Boot/meridian/programs`, the loader's own folder on the
/// volume it came from, which Meridian never writes. Names are compared
/// in capitals, as the tree of files compares them.
#[must_use]
pub fn loaders_copy(path: &str) -> Option<&str> {
    let path = crate::vfs::Path::parse(path)?;
    match path.names() {
        [system, boot, meridian, programs, name]
            if system.eq_ignore_ascii_case("System")
                && boot.eq_ignore_ascii_case("Boot")
                && meridian.eq_ignore_ascii_case("meridian")
                && programs.eq_ignore_ascii_case("programs") =>
        {
            Some(name)
        }
        _ => None,
    }
}

/// Whether the file `module` lies in the kernel's own memory, where the
/// loader puts what it reads.
#[must_use]
pub fn module_kept(module: &Module, regions: &[MemoryRegion]) -> bool {
    crate::covered(regions, RegionKind::KERNEL, module.start, module.size)
}

/// Where a program whose image spans `span`, the first page as linked and
/// the size, starts in its address space, given its `entry` as linked and
/// its `segments`; or why it may not run.
///
/// # Errors
///
/// When the image is too big, a page is to be writable and executable, or
/// the entry is not on an executable page of the image.
pub fn place(span: (u64, u64), entry: u64, segments: &[Segment]) -> Result<u64, Refusal> {
    let (first, size) = span;
    if size > IMAGE_MAX {
        return Err(Refusal::TooBig(size));
    }
    for page in (0..size.div_ceil(PAGE)).map(|i| first + i * PAGE) {
        page_flags(segments, page)?;
    }
    let inside = (first..first + size).contains(&entry);
    if !inside || !elf::access(segments, entry).is_some_and(|a| a.execute) {
        return Err(Refusal::BadEntry(entry));
    }
    Ok(entry - first + CODE_AT)
}

/// The flags the page at `linked` is mapped with in the program's address
/// space, or none where no segment lies and the page stays unmapped.
///
/// # Errors
///
/// When the page is to be writable and executable.
pub fn page_flags(segments: &[Segment], linked: u64) -> Result<Option<Flags>, Refusal> {
    let Some(access) = elf::access(segments, linked) else {
        return Ok(None);
    };
    if access.write && access.execute {
        return Err(Refusal::WritableCode(linked & !(PAGE - 1)));
    }
    let mut flags = Flags::USER;
    if access.write {
        flags = flags | Flags::WRITABLE;
    }
    if !access.execute {
        flags = flags | Flags::NO_EXECUTE;
    }
    Ok(Some(flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u64, pages: u64, kind: RegionKind) -> MemoryRegion {
        MemoryRegion {
            start,
            pages,
            kind,
            reserved: 0,
        }
    }

    fn listing(list: u64, count: u64) -> BootInfo {
        BootInfo {
            modules: Modules { list, count },
            ..BootInfo::new()
        }
    }

    #[test]
    fn only_a_path_into_the_loaders_folder_is_one_of_its_copies() {
        assert_eq!(
            loaders_copy("/System/Boot/meridian/programs/wc.mexec"),
            Some("wc.mexec")
        );
        assert_eq!(
            loaders_copy("/system/BOOT/Meridian/PROGRAMS/WC.MEXEC"),
            Some("WC.MEXEC"),
            "in any capitals"
        );
        assert_eq!(
            loaders_copy("/System/Boot/meridian/x/../programs/./wc.mexec"),
            Some("wc.mexec"),
            "as the tree finds it"
        );
        for elsewhere in [
            "/System/meridian/programs/wc.mexec",
            "/System/Boot/meridian/programs",
            "/System/Boot/meridian/programs/a/wc.mexec",
            "/Other/Boot/meridian/programs/wc.mexec",
            "/System/Boot/meridian/program/wc.mexec",
            "wc.mexec",
        ] {
            assert_eq!(loaders_copy(elsewhere), None, "{elsewhere}");
        }
    }

    #[test]
    fn the_list_of_files_is_read_only_from_the_kernels_memory() {
        let regions = [
            region(0x10_0000, 0x10, RegionKind::USABLE),
            region(0x20_0000, 0x10, RegionKind::KERNEL),
        ];
        let at = HHDM_BASE + 0x20_3000;
        assert_eq!(module_list(&listing(at, 2), &regions), Some((0x20_3000, 2)));
        let most = Modules::MAX as u64;
        assert_eq!(
            module_list(&listing(at, most), &regions),
            Some((0x20_3000, Modules::MAX)),
            "as many as there may be"
        );
        assert_eq!(
            module_list(&listing(at, most + 1), &regions),
            None,
            "more than the loader writes"
        );
        assert_eq!(
            module_list(&listing(at, 0), &regions),
            None,
            "nothing to read"
        );
        assert_eq!(
            module_list(&listing(HHDM_BASE + 0x10_0000, 1), &regions),
            None,
            "usable memory"
        );
        assert_eq!(
            module_list(&listing(0x20_3000, 1), &regions),
            None,
            "not in the direct map"
        );
        assert_eq!(module_list(&listing(0, 1), &regions), None);
        let edge = HHDM_BASE + 0x20_f000 + 4096 - 48;
        assert_eq!(
            module_list(&listing(edge, 1), &regions),
            Some((0x20_f000 + 4096 - 48, 1))
        );
        assert_eq!(
            module_list(&listing(edge, 2), &regions),
            None,
            "running past the region"
        );
        assert_eq!(module_list(&listing(u64::MAX, 1), &regions), None);
    }

    #[test]
    fn a_file_is_trusted_only_where_the_loader_puts_files() {
        let regions = [
            region(0x10_0000, 0x10, RegionKind::USABLE),
            region(0x20_0000, 0x10, RegionKind::KERNEL),
        ];
        let module = |start, size| Module::new(start, size, b"hello").unwrap();
        assert!(module_kept(&module(0x20_0000, 0x10_000), &regions));
        assert!(
            module_kept(&module(0x20_8000, 0x8000), &regions),
            "right to its end"
        );
        assert!(!module_kept(&module(0x20_8000, 0x8001), &regions));
        assert!(
            !module_kept(&module(0x10_0000, 0x10), &regions),
            "usable memory"
        );
        assert!(!module_kept(&module(u64::MAX - 1, 0x10), &regions));
    }

    fn segment(vaddr: u64, mem_size: u64, write: bool, execute: bool) -> Segment {
        Segment {
            vaddr,
            mem_size,
            file_offset: 0,
            file_size: 0,
            read: true,
            write,
            execute,
        }
    }

    /// The way the linker lays out a small program: read-only data, code,
    /// then data, each on pages of its own.
    fn program() -> [Segment; 3] {
        [
            segment(0, 0x16e0, false, false),
            segment(0x26e0, 0x37a8, false, true),
            segment(0x6e88, 0x210, true, false),
        ]
    }

    fn note(owner: &'static [u8], kind: u32, desc: &'static [u8]) -> Note<'static> {
        Note { owner, kind, desc }
    }

    #[test]
    fn a_meridian_program_says_so_in_a_note() {
        let notes = [
            note(b"GNU", 1, &[1, 0, 0, 0]),
            note(b"Meridian", 1, &[1, 0, 0, 0]),
            note(b"Meridian", 2, b"hello"),
            note(b"Meridian", 77, b"a kind to come"),
        ];
        assert_eq!(metadata(notes.into_iter()), Ok(Metadata { name: b"hello" }));
        let nameless = [note(b"Meridian", 1, &[1, 0, 0, 0])];
        assert_eq!(metadata(nameless.into_iter()), Ok(Metadata { name: b"" }));
    }

    #[test]
    fn a_file_without_the_note_is_no_meridian_program() {
        assert_eq!(metadata([].into_iter()), Err(Refusal::NotMeridian));
        let others = [
            note(b"GNU", 1, &[1, 0, 0, 0]),
            note(b"Meridian", 2, b"hello"),
        ];
        assert_eq!(
            metadata(others.into_iter()),
            Err(Refusal::NotMeridian),
            "a name alone, or another owner's kind 1, says nothing"
        );
        let later = [note(b"Meridian", 1, &[2, 0, 0, 0])];
        assert_eq!(metadata(later.into_iter()), Err(Refusal::Version(2)));
        let broken = [note(b"Meridian", 1, &[1, 0, 0])];
        assert_eq!(
            metadata(broken.into_iter()),
            Err(Refusal::Version(0)),
            "a version in three bytes is none"
        );
        let owner = [note(b"Meridian\0", 1, &[1, 0, 0, 0])];
        assert_eq!(
            metadata(owner.into_iter()),
            Err(Refusal::NotMeridian),
            "lib/elf takes the zero off the owner"
        );
    }

    #[test]
    fn memory_is_given_out_one_piece_after_another() {
        let mut heap = Heap::new();
        assert_eq!(heap.take(1), Ok(HEAP_START));
        assert_eq!(heap.take(3), Ok(HEAP_START + PAGE));
        assert_eq!(heap.take(1), Ok(HEAP_START + 4 * PAGE));
        assert_eq!(
            heap.take(MAP_MAX),
            Ok(HEAP_START + 5 * PAGE),
            "as much as one call gives"
        );
        assert_eq!(Heap::default(), Heap::new());
    }

    #[test]
    fn memory_is_given_only_in_sizes_there_are() {
        let mut heap = Heap::new();
        assert_eq!(heap.take(0), Err(abi::Error::BadSize));
        assert_eq!(heap.take(MAP_MAX + 1), Err(abi::Error::BadSize));
        assert_eq!(heap.take(u64::MAX), Err(abi::Error::BadSize));
        assert_eq!(heap.take(1), Ok(HEAP_START), "refusals give nothing out");
    }

    #[test]
    fn the_region_ends_where_it_ends() {
        let mut heap = Heap::new();
        let calls = (HEAP_END - HEAP_START) / (MAP_MAX * PAGE);
        for _ in 0..calls - 1 {
            heap.take(MAP_MAX).unwrap();
        }
        assert_eq!(heap.take(MAP_MAX - 1), Ok(HEAP_END - MAP_MAX * PAGE));
        assert_eq!(heap.take(1), Ok(HEAP_END - PAGE), "the last page");
        assert_eq!(heap.take(1), Err(abi::Error::NoRoom));
        assert!(heap.holds(HEAP_END - PAGE, 1));
        assert!(!heap.holds(HEAP_END - PAGE, 2));
    }

    #[test]
    fn only_what_was_given_can_come_back() {
        let mut heap = Heap::new();
        assert!(!heap.holds(HEAP_START, 1), "nothing given yet");
        heap.take(4).unwrap();
        assert!(heap.holds(HEAP_START, 4));
        assert!(heap.holds(HEAP_START + PAGE, 2), "a part of it");
        assert!(heap.holds(HEAP_START + 3 * PAGE, 1), "its last page");
        assert!(!heap.holds(HEAP_START + 3 * PAGE, 2), "past what was given");
        assert!(!heap.holds(HEAP_START + 1, 1), "not on a page");
        assert!(!heap.holds(HEAP_START, 0));
        assert!(!heap.holds(HEAP_START - PAGE, 1), "below the region");
        assert!(!heap.holds(CODE_AT, 1), "the image");
        assert!(!heap.holds(HEAP_START, u64::MAX));
        assert!(!heap.holds(!(PAGE - 1), 1), "the last page of all");
    }

    #[test]
    fn a_program_starts_where_its_entry_lands_in_its_space() {
        assert_eq!(place((0, 0x8000), 0x26e0, &program()), Ok(CODE_AT + 0x26e0));
        let moved = program().map(|s| Segment {
            vaddr: s.vaddr + 0x10_0000,
            ..s
        });
        assert_eq!(
            place((0x10_0000, 0x8000), 0x10_26e0, &moved),
            Ok(CODE_AT + 0x26e0),
            "linked elsewhere, it still starts at its image's spot"
        );
    }

    #[test]
    fn a_program_starts_only_in_its_code() {
        let segments = program();
        assert_eq!(
            place((0, 0x8000), 0x100, &segments),
            Err(Refusal::BadEntry(0x100)),
            "data"
        );
        assert_eq!(
            place((0, 0x8000), 0x6f00, &segments),
            Err(Refusal::BadEntry(0x6f00))
        );
        assert_eq!(
            place((0, 0x8000), 0x9000, &segments),
            Err(Refusal::BadEntry(0x9000)),
            "past the image"
        );
        assert_eq!(
            place((0x1000, 0x7000), 0x26e0, &segments[1..]),
            Ok(CODE_AT + 0x16e0)
        );
        assert_eq!(
            place((0x3000, 0x5000), 0x26e0, &segments),
            Err(Refusal::BadEntry(0x26e0)),
            "code, but before the image"
        );
        assert_eq!(
            place((0, 0x8000), 0x5e87, &segments),
            Ok(CODE_AT + 0x5e87),
            "the code's last byte"
        );
    }

    #[test]
    fn an_image_has_to_fit_above_the_stack() {
        assert_eq!(IMAGE_MAX, 0x3e_f000);
        assert_eq!(
            CODE_AT + IMAGE_MAX,
            STACK_TOP - STACK_PAGES * PAGE - PAGE,
            "the guard page stays"
        );
        let code = [segment(0, IMAGE_MAX, false, true)];
        assert_eq!(place((0, IMAGE_MAX), 0, &code), Ok(CODE_AT));
        let more = [segment(0, IMAGE_MAX + 1, false, true)];
        assert_eq!(
            place((0, IMAGE_MAX + PAGE), 0, &more),
            Err(Refusal::TooBig(IMAGE_MAX + PAGE))
        );
    }

    #[test]
    fn no_page_is_writable_and_executable() {
        let shared = [
            segment(0, 0x800, false, true),
            segment(0x800, 0x800, true, false),
        ];
        assert_eq!(page_flags(&shared, 0x10), Err(Refusal::WritableCode(0)));
        assert_eq!(
            place((0, 0x1000), 0, &shared),
            Err(Refusal::WritableCode(0))
        );
        let later = [
            segment(0, 0x1000, false, true),
            segment(0x2000, 0x800, false, true),
            segment(0x2800, 0x800, true, false),
        ];
        assert_eq!(
            place((0, 0x3000), 0, &later),
            Err(Refusal::WritableCode(0x2000)),
            "whichever page it is"
        );
        assert_eq!(
            page_flags(&later, 0x2abc),
            Err(Refusal::WritableCode(0x2000))
        );
    }

    #[test]
    fn each_page_is_mapped_for_what_it_holds() {
        let segments = program();
        let data = Flags::USER | Flags::NO_EXECUTE;
        assert_eq!(page_flags(&segments, 0), Ok(Some(data)), "read-only data");
        assert_eq!(page_flags(&segments, 0x1000), Ok(Some(data)));
        assert_eq!(page_flags(&segments, 0x2000), Ok(Some(Flags::USER)), "code");
        assert_eq!(page_flags(&segments, 0x5000), Ok(Some(Flags::USER)));
        assert_eq!(
            page_flags(&segments, 0x6000),
            Ok(Some(Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE)),
            "data"
        );
        assert_eq!(
            page_flags(&segments, 0x7000),
            page_flags(&segments, 0x6000),
            "the data's second page"
        );
        assert_eq!(page_flags(&segments, 0x8000), Ok(None), "past the end");
        let gap = [
            segment(0, 0x1000, false, true),
            segment(0x3000, 0x10, true, false),
        ];
        assert_eq!(page_flags(&gap, 0x1000), Ok(None), "between segments");
        assert_eq!(place((0, 0x4000), 0, &gap), Ok(CODE_AT));
    }
}
