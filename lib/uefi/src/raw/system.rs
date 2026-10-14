//! Safe access to the system table while boot services are running.

use core::cell::Cell;
use core::fmt;
use core::ptr::{self, NonNull};

use super::Handle;
use super::console::SimpleTextOutput;
use super::table::{
    AllocateType, BOOT_SERVICES_SIGNATURE, BootServices, SYSTEM_TABLE_SIGNATURE, SystemTable,
    TableHeader, header_is_valid,
};
use crate::memory::{MemoryMap, MemoryType, PAGE_BYTES};
use crate::text::{InputKey, Ucs2, decode_ucs2};
use crate::{Guid, Status};

/// Longest firmware vendor name read, in UCS-2 units. Real names are short;
/// the limit only guards against a missing terminator.
const VENDOR_LIMIT: usize = 128;

/// The firmware's system table, checked once at entry. A `System` is only
/// ever created from the pointer the firmware passes to the entry point.
pub struct System {
    image: Handle,
    table: NonNull<SystemTable>,
    /// Whether a [`super::graphics::Pixels`] is out, so there is only ever
    /// one way to write the framebuffer at a time.
    pub(super) pixels_out: Cell<bool>,
}

impl System {
    /// Wraps the system table handed to the image entry point. Returns
    /// `None` when the table or its boot services table does not carry the
    /// signature and size the specification requires.
    ///
    /// # Safety
    ///
    /// `table` must be the pointer the firmware passed to the entry point,
    /// and boot services must not have been exited.
    #[must_use]
    pub unsafe fn from_entry(image: Handle, table: *mut SystemTable) -> Option<System> {
        let table = NonNull::new(table)?;
        // SAFETY: the caller promises the pointer is the firmware's own table.
        let st = unsafe { table.as_ref() };
        if !header_is_valid(&st.hdr, SYSTEM_TABLE_SIGNATURE, size_of::<SystemTable>()) {
            return None;
        }
        let bs = st.boot_services;
        if bs.is_null() {
            return None;
        }
        // SAFETY: a valid system table points at the firmware's boot
        // services. Only the header is read until it has been checked.
        let hdr = unsafe { &(*bs).hdr };
        if !header_is_valid(hdr, BOOT_SERVICES_SIGNATURE, size_of::<BootServices>()) {
            return None;
        }
        Some(System {
            image,
            table,
            pixels_out: Cell::new(false),
        })
    }

    pub(super) fn table(&self) -> &SystemTable {
        // SAFETY: checked in from_entry; the firmware keeps the table alive
        // for as long as boot services run, which is as long as Meridian
        // holds a System.
        unsafe { self.table.as_ref() }
    }

    pub(super) fn boot_services(&self) -> &BootServices {
        // SAFETY: checked non-null and valid in from_entry.
        unsafe { &*self.table().boot_services }
    }

    #[must_use]
    pub fn image(&self) -> Handle {
        self.image
    }

    /// The physical address of the system table, which the kernel keeps
    /// for the runtime services.
    #[must_use]
    pub fn table_address(&self) -> u64 {
        self.table.as_ptr() as u64
    }

    /// Ends boot services, which hands the machine to the loader for good.
    /// `map_key` must be the key of the memory map read last; when the map
    /// has changed since, the firmware refuses and the `System` comes back
    /// to read it again.
    ///
    /// # Errors
    ///
    /// Returns the `System` and the firmware's status, `INVALID_PARAMETER`
    /// for a stale key.
    pub fn exit_boot_services(self, map_key: usize) -> Result<(), (System, Status)> {
        // SAFETY: self is consumed, so nothing can call boot services
        // after this, and the image handle is the one the entry point got.
        let status = unsafe { (self.boot_services().exit_boot_services)(self.image, map_key) };
        match status.ok() {
            Ok(()) => Ok(()),
            Err(status) => Err((self, status)),
        }
    }

    #[must_use]
    pub fn header(&self) -> &TableHeader {
        &self.table().hdr
    }

    #[must_use]
    pub fn firmware_revision(&self) -> u32 {
        self.table().firmware_revision
    }

    /// The firmware vendor, such as "EDK II".
    pub fn firmware_vendor(&self) -> impl Iterator<Item = char> + '_ {
        let start = self.table().firmware_vendor;
        let mut len = 0;
        if !start.is_null() {
            // SAFETY: the vendor string is NUL-terminated firmware memory;
            // the scan stops at the terminator or the limit.
            while len < VENDOR_LIMIT && unsafe { *start.add(len) } != 0 {
                len += 1;
            }
        }
        let units: &[u16] = if len == 0 {
            &[]
        } else {
            // SAFETY: the first len units were just read one by one.
            unsafe { core::slice::from_raw_parts(start, len) }
        };
        decode_ucs2(units)
    }

    /// The address of the configuration table filed under `guid`, such as
    /// the ACPI root pointer, or `None` when the firmware has none.
    #[must_use]
    pub fn configuration_table(&self, guid: &Guid) -> Option<u64> {
        let st = self.table();
        if st.configuration_table.is_null() {
            return None;
        }
        // SAFETY: the firmware keeps number_of_table_entries entries at
        // configuration_table for as long as boot services run.
        let entries = unsafe {
            core::slice::from_raw_parts(st.configuration_table, st.number_of_table_entries)
        };
        entries
            .iter()
            .find(|e| e.vendor_guid == *guid)
            .map(|e| e.vendor_table as u64)
    }

    /// The firmware's text console, or `None` on firmware without one.
    #[must_use]
    pub fn console(&self) -> Option<Console<'_>> {
        let out = self.table().con_out;
        if out.is_null() {
            return None;
        }
        Some(Console { out, _system: self })
    }

    /// Waits for at least `microseconds`.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status if it refuses.
    pub fn stall(&self, microseconds: usize) -> Result<(), Status> {
        // SAFETY: Stall takes a plain number.
        unsafe { (self.boot_services().stall)(microseconds) }.ok()
    }

    /// Allocates `count` contiguous pages of `ty` anywhere in memory and
    /// clears them. They stay allocated when the returned [`Pages`] is
    /// dropped; only [`Pages::free`] gives them back.
    ///
    /// # Errors
    ///
    /// `INVALID_PARAMETER` for no pages at all, otherwise the firmware's
    /// status, such as `OUT_OF_RESOURCES`.
    pub fn allocate_pages(&self, ty: MemoryType, count: usize) -> Result<Pages<'_>, Status> {
        if count == 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        let mut start = 0;
        // SAFETY: with ANY_PAGES the firmware only writes the address it
        // chose into start.
        unsafe {
            (self.boot_services().allocate_pages)(
                AllocateType::ANY_PAGES,
                ty,
                count,
                &raw mut start,
            )
        }
        .ok()?;
        let mut pages = Pages {
            start,
            count,
            system: self,
        };
        pages.bytes().fill(0);
        Ok(pages)
    }

    /// How many bytes the memory map takes right now. Allocating a buffer
    /// for it adds a descriptor or two, so leave some room.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status if it cannot say.
    pub fn memory_map_size(&self) -> Result<usize, Status> {
        let (mut size, mut key, mut descriptor_size, mut version) = (0, 0, 0, 0);
        // SAFETY: a size of 0 makes the firmware only fill in the numbers.
        let status = unsafe {
            (self.boot_services().get_memory_map)(
                &raw mut size,
                ptr::null_mut(),
                &raw mut key,
                &raw mut descriptor_size,
                &raw mut version,
            )
        };
        match status {
            Status::SUCCESS | Status::BUFFER_TOO_SMALL => Ok(size),
            other => Err(other),
        }
    }

    /// Writes the current memory map into `buf`.
    ///
    /// # Errors
    ///
    /// `BUFFER_TOO_SMALL` when the map does not fit, which
    /// [`System::memory_map_size`] avoids.
    pub fn memory_map<'b>(&self, buf: &'b mut [u8]) -> Result<MemoryMap<'b>, Status> {
        let (mut size, mut key, mut descriptor_size, mut version) = (buf.len(), 0, 0, 0);
        // SAFETY: the firmware writes at most size bytes into buf.
        unsafe {
            (self.boot_services().get_memory_map)(
                &raw mut size,
                buf.as_mut_ptr(),
                &raw mut key,
                &raw mut descriptor_size,
                &raw mut version,
            )
        }
        .ok()?;
        Ok(MemoryMap {
            bytes: &buf[..size.min(buf.len())],
            key,
            descriptor_size,
            descriptor_version: version,
        })
    }

    /// Blocks until a key is pressed and returns it.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status when there is no keyboard input or
    /// waiting fails.
    pub fn wait_for_key(&self) -> Result<InputKey, Status> {
        let input = self.table().con_in;
        if input.is_null() {
            return Err(Status::UNSUPPORTED);
        }
        // SAFETY: con_in is the firmware's protocol instance, non-null here.
        let event = unsafe { (*input).wait_for_key };
        let mut index = 0;
        // SAFETY: one valid event and a place for the index.
        unsafe { (self.boot_services().wait_for_event)(1, &raw const event, &raw mut index) }
            .ok()?;
        let mut key = InputKey::default();
        // SAFETY: the key event fired, so a key stroke is waiting.
        unsafe { ((*input).read_key_stroke)(input, &raw mut key) }.ok()?;
        Ok(key)
    }

    /// The next key stroke if one is waiting, without waiting for one.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status when there is no keyboard input or
    /// reading fails.
    pub fn read_key(&self) -> Result<Option<InputKey>, Status> {
        let input = self.table().con_in;
        if input.is_null() {
            return Err(Status::UNSUPPORTED);
        }
        let mut key = InputKey::default();
        // SAFETY: con_in is the firmware's protocol instance, non-null here,
        // and ReadKeyStroke only writes the key.
        match unsafe { ((*input).read_key_stroke)(input, &raw mut key) } {
            Status::NOT_READY => Ok(None),
            status => status.ok().map(|()| Some(key)),
        }
    }
}

/// Pages the firmware allocated. Boot services run with memory identity
/// mapped, so the physical address of the pages is also where they are.
pub struct Pages<'a> {
    start: u64,
    count: usize,
    system: &'a System,
}

impl Pages<'_> {
    /// The physical address of the first page.
    #[must_use]
    pub fn start(&self) -> u64 {
        self.start
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn bytes(&mut self) -> &mut [u8] {
        let len = self.count * PAGE_BYTES;
        // SAFETY: the firmware gave these pages to this value alone, and
        // they are identity mapped while boot services run.
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, len) }
    }

    /// The pages as 32-bit words, such as pixels.
    pub fn words(&mut self) -> &mut [u32] {
        let len = self.count * PAGE_BYTES / 4;
        // SAFETY: the pages are page aligned, so aligned for u32, belong to
        // this value alone, and every bit pattern is a valid u32.
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u32, len) }
    }

    /// Keeps the pages for as long as the loader runs, as bytes that no
    /// longer borrow the `System`, so they can still be written while
    /// boot services are being left. The firmware never takes them back.
    #[must_use]
    pub fn keep(mut self) -> &'static mut [u8] {
        let len = self.bytes().len();
        // SAFETY: the pages stay allocated, nobody else got them, and the
        // loader neither frees them nor changes the identity map before it
        // stops running.
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, len) }
    }

    /// Gives the pages back to the firmware.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status if it refuses.
    pub fn free(self) -> Result<(), Status> {
        // SAFETY: start and count are exactly what AllocatePages returned.
        unsafe { (self.system.boot_services().free_pages)(self.start, self.count) }.ok()
    }
}

/// The firmware text console. Writing goes through `core::fmt::Write`.
pub struct Console<'a> {
    out: *mut SimpleTextOutput,
    _system: &'a System,
}

impl Console<'_> {
    /// Clears the screen and puts the cursor at the top left.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status if clearing fails.
    pub fn clear(&mut self) -> Result<(), Status> {
        // SAFETY: out is the firmware's console, valid while System lives.
        unsafe { ((*self.out).clear_screen)(self.out) }.ok()
    }
}

impl fmt::Write for Console<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut encoder = Ucs2::new(s);
        let mut buf = [0u16; 128];
        while encoder.fill(&mut buf) > 0 {
            // SAFETY: buf is NUL-terminated by fill, out is the firmware's
            // console and valid while System lives.
            let status = unsafe { ((*self.out).output_string)(self.out, buf.as_ptr()) };
            if status.is_error() {
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

impl fmt::Debug for System {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("System")
            .field("image", &self.image)
            .field("table", &ptr::from_ref(self.table()))
            .field("pixels_out", &self.pixels_out.get())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::table::{BOOT_SERVICES_SIGNATURE, SYSTEM_TABLE_SIGNATURE};
    use core::mem::{MaybeUninit, size_of};

    fn header(signature: u64, size: usize) -> TableHeader {
        TableHeader {
            signature,
            revision: (2 << 16) + 70,
            header_size: u32::try_from(size).unwrap(),
            crc32: 0,
            reserved: 0,
        }
    }

    /// A boot services table with only its header filled in. Nothing in
    /// these tests calls into it, and since its function slots hold no
    /// functions it is never turned into a reference either.
    fn boot_services(signature: u64, size: usize) -> MaybeUninit<BootServices> {
        let mut bs = MaybeUninit::<BootServices>::zeroed();
        // SAFETY: writes the header of memory owned right here.
        unsafe { (&raw mut (*bs.as_mut_ptr()).hdr).write(header(signature, size)) };
        bs
    }

    fn system_table(
        signature: u64,
        size: usize,
        bs: *mut BootServices,
        vendor: *const u16,
    ) -> SystemTable {
        SystemTable {
            hdr: header(signature, size),
            firmware_vendor: vendor,
            firmware_revision: 0x1_0000,
            console_in_handle: core::ptr::null_mut(),
            con_in: core::ptr::null_mut(),
            console_out_handle: core::ptr::null_mut(),
            con_out: core::ptr::null_mut(),
            standard_error_handle: core::ptr::null_mut(),
            std_err: core::ptr::null_mut(),
            runtime_services: core::ptr::null_mut(),
            boot_services: bs,
            number_of_table_entries: 0,
            configuration_table: core::ptr::null_mut(),
        }
    }

    fn open(st: &mut SystemTable) -> Option<System> {
        // SAFETY: st outlives the System in every test, and nothing is called.
        unsafe { System::from_entry(core::ptr::null_mut(), st) }
    }

    const ST_SIZE: usize = size_of::<SystemTable>();
    const BS_SIZE: usize = size_of::<BootServices>();

    #[test]
    fn accepts_a_well_formed_table() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        let at = (&raw const st) as u64;
        let system = open(&mut st).expect("a valid table");
        assert_eq!(system.firmware_revision(), 0x1_0000);
        assert_eq!(system.header().revision, (2 << 16) + 70);
        assert!(system.image().is_null());
        assert_eq!(system.table_address(), at);
    }

    #[test]
    fn refuses_null_and_wrong_tables() {
        assert!(
            // SAFETY: a null table is exactly what is being refused.
            unsafe { System::from_entry(core::ptr::null_mut(), core::ptr::null_mut()) }.is_none()
        );

        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut wrong_sig = system_table(
            BOOT_SERVICES_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        assert!(open(&mut wrong_sig).is_none());

        let mut too_small = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE - 1,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        assert!(open(&mut too_small).is_none());

        let mut no_bs = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            core::ptr::null_mut(),
            core::ptr::null(),
        );
        assert!(open(&mut no_bs).is_none());
    }

    #[test]
    fn refuses_broken_boot_services() {
        let mut wrong = boot_services(SYSTEM_TABLE_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            wrong.as_mut_ptr(),
            core::ptr::null(),
        );
        assert!(open(&mut st).is_none());

        let mut short = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE - 8);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            short.as_mut_ptr(),
            core::ptr::null(),
        );
        assert!(open(&mut st).is_none());
    }

    #[test]
    fn vendor_is_read_up_to_the_terminator() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let name: Vec<u16> = "EDK II\0garbage".encode_utf16().collect();
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            name.as_ptr(),
        );
        let system = open(&mut st).unwrap();
        assert_eq!(system.firmware_vendor().collect::<String>(), "EDK II");
    }

    #[test]
    fn vendor_names_outside_the_basic_plane_decode() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let name: Vec<u16> = "Firm\u{1F600}ware\0".encode_utf16().collect();
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            name.as_ptr(),
        );
        let system = open(&mut st).unwrap();
        assert_eq!(
            system.firmware_vendor().collect::<String>(),
            "Firm\u{1F600}ware"
        );
    }

    #[test]
    fn vendor_without_terminator_stops_at_the_limit() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let name = vec![u16::from(b'x'); VENDOR_LIMIT + 50];
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            name.as_ptr(),
        );
        let system = open(&mut st).unwrap();
        assert_eq!(system.firmware_vendor().count(), VENDOR_LIMIT);
    }

    #[test]
    fn missing_or_empty_vendor_is_empty() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        assert_eq!(open(&mut st).unwrap().firmware_vendor().count(), 0);
        let empty = [0u16];
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            empty.as_ptr(),
        );
        assert_eq!(open(&mut st).unwrap().firmware_vendor().count(), 0);
    }

    #[test]
    fn no_console_and_no_keyboard_are_reported_without_calls() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        let system = open(&mut st).unwrap();
        assert!(system.console().is_none());
        assert_eq!(system.wait_for_key(), Err(Status::UNSUPPORTED));
        assert_eq!(system.read_key(), Err(Status::UNSUPPORTED));
    }

    #[test]
    fn configuration_tables_are_found_by_guid() {
        use crate::guid::{ACPI_20_TABLE, SMBIOS3_TABLE};
        use crate::raw::table::ConfigurationTable;

        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        let mut entries = [
            ConfigurationTable {
                vendor_guid: SMBIOS3_TABLE,
                vendor_table: 0x7f00_0000 as *mut _,
            },
            ConfigurationTable {
                vendor_guid: ACPI_20_TABLE,
                vendor_table: 0x7e00_1000 as *mut _,
            },
        ];
        st.configuration_table = entries.as_mut_ptr();
        st.number_of_table_entries = 2;
        let system = open(&mut st).unwrap();
        assert_eq!(
            system.configuration_table(&ACPI_20_TABLE),
            Some(0x7e00_1000)
        );
        assert_eq!(
            system.configuration_table(&SMBIOS3_TABLE),
            Some(0x7f00_0000)
        );
        assert_eq!(system.configuration_table(&crate::guid::FILE_INFO), None);

        st.number_of_table_entries = 1;
        let system = open(&mut st).unwrap();
        assert_eq!(
            system.configuration_table(&ACPI_20_TABLE),
            None,
            "past the count"
        );
        st.number_of_table_entries = 2;
        st.configuration_table = core::ptr::null_mut();
        let system = open(&mut st).unwrap();
        assert_eq!(
            system.configuration_table(&ACPI_20_TABLE),
            None,
            "no table at all"
        );
    }

    #[test]
    fn no_pages_is_refused_before_asking_the_firmware() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            bs.as_mut_ptr(),
            core::ptr::null(),
        );
        let system = open(&mut st).unwrap();
        assert_eq!(
            system.allocate_pages(MemoryType::LOADER_DATA, 0).err(),
            Some(Status::INVALID_PARAMETER)
        );
    }
}
