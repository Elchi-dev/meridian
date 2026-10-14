//! Safe access to the system table while boot services are running.

use core::fmt;
use core::ptr::{self, NonNull};

use super::Handle;
use super::console::SimpleTextOutput;
use super::table::{
    BOOT_SERVICES_SIGNATURE, BootServices, SYSTEM_TABLE_SIGNATURE, SystemTable, TableHeader,
    header_is_valid,
};
use crate::Status;
use crate::text::{InputKey, Ucs2, decode_ucs2};

/// Longest firmware vendor name read, in UCS-2 units. Real names are short;
/// the limit only guards against a missing terminator.
const VENDOR_LIMIT: usize = 128;

/// The firmware's system table, checked once at entry. A `System` is only
/// ever created from the pointer the firmware passes to the entry point.
pub struct System {
    image: Handle,
    table: NonNull<SystemTable>,
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
        // SAFETY: a valid system table points at the firmware's boot services.
        let bs = unsafe { st.boot_services.as_ref()? };
        if !header_is_valid(&bs.hdr, BOOT_SERVICES_SIGNATURE, size_of::<BootServices>()) {
            return None;
        }
        Some(System { image, table })
    }

    fn table(&self) -> &SystemTable {
        // SAFETY: checked in from_entry; the firmware keeps the table alive
        // for as long as boot services run, which is as long as Meridian
        // holds a System.
        unsafe { self.table.as_ref() }
    }

    fn boot_services(&self) -> &BootServices {
        // SAFETY: checked non-null and valid in from_entry.
        unsafe { &*self.table().boot_services }
    }

    #[must_use]
    pub fn image(&self) -> Handle {
        self.image
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
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::table::{BOOT_SERVICES_SIGNATURE, SYSTEM_TABLE_SIGNATURE};
    use core::mem::{MaybeUninit, size_of};

    // The validation below never calls into the tables, but their typed
    // slots need some function. These say so loudly if that ever changes.
    unsafe extern "efiapi" fn never_stall(_: usize) -> Status {
        panic!("validation must not call Stall")
    }
    unsafe extern "efiapi" fn never_wait(
        _: usize,
        _: *const super::super::Event,
        _: *mut usize,
    ) -> Status {
        panic!("validation must not call WaitForEvent")
    }

    fn header(signature: u64, size: usize) -> TableHeader {
        TableHeader {
            signature,
            revision: (2 << 16) + 70,
            header_size: u32::try_from(size).unwrap(),
            crc32: 0,
            reserved: 0,
        }
    }

    fn boot_services(signature: u64, size: usize) -> BootServices {
        let mut bs = MaybeUninit::<BootServices>::zeroed();
        let p = bs.as_mut_ptr();
        // SAFETY: zero is a valid value for every field except the two
        // function pointers, and both are written before assume_init.
        unsafe {
            (&raw mut (*p).hdr).write(header(signature, size));
            (&raw mut (*p).stall).write(never_stall);
            (&raw mut (*p).wait_for_event).write(never_wait);
            bs.assume_init()
        }
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
            &raw mut bs,
            core::ptr::null(),
        );
        let system = open(&mut st).expect("a valid table");
        assert_eq!(system.firmware_revision(), 0x1_0000);
        assert_eq!(system.header().revision, (2 << 16) + 70);
        assert!(system.image().is_null());
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
            &raw mut bs,
            core::ptr::null(),
        );
        assert!(open(&mut wrong_sig).is_none());

        let mut too_small = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE - 1,
            &raw mut bs,
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
            &raw mut wrong,
            core::ptr::null(),
        );
        assert!(open(&mut st).is_none());

        let mut short = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE - 8);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            &raw mut short,
            core::ptr::null(),
        );
        assert!(open(&mut st).is_none());
    }

    #[test]
    fn vendor_is_read_up_to_the_terminator() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let name: Vec<u16> = "EDK II\0garbage".encode_utf16().collect();
        let mut st = system_table(SYSTEM_TABLE_SIGNATURE, ST_SIZE, &raw mut bs, name.as_ptr());
        let system = open(&mut st).unwrap();
        assert_eq!(system.firmware_vendor().collect::<String>(), "EDK II");
    }

    #[test]
    fn vendor_names_outside_the_basic_plane_decode() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let name: Vec<u16> = "Firm\u{1F600}ware\0".encode_utf16().collect();
        let mut st = system_table(SYSTEM_TABLE_SIGNATURE, ST_SIZE, &raw mut bs, name.as_ptr());
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
        let mut st = system_table(SYSTEM_TABLE_SIGNATURE, ST_SIZE, &raw mut bs, name.as_ptr());
        let system = open(&mut st).unwrap();
        assert_eq!(system.firmware_vendor().count(), VENDOR_LIMIT);
    }

    #[test]
    fn missing_or_empty_vendor_is_empty() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            &raw mut bs,
            core::ptr::null(),
        );
        assert_eq!(open(&mut st).unwrap().firmware_vendor().count(), 0);
        let empty = [0u16];
        let mut st = system_table(SYSTEM_TABLE_SIGNATURE, ST_SIZE, &raw mut bs, empty.as_ptr());
        assert_eq!(open(&mut st).unwrap().firmware_vendor().count(), 0);
    }

    #[test]
    fn no_console_and_no_keyboard_are_reported_without_calls() {
        let mut bs = boot_services(BOOT_SERVICES_SIGNATURE, BS_SIZE);
        let mut st = system_table(
            SYSTEM_TABLE_SIGNATURE,
            ST_SIZE,
            &raw mut bs,
            core::ptr::null(),
        );
        let system = open(&mut st).unwrap();
        assert!(system.console().is_none());
        assert_eq!(system.wait_for_key(), Err(Status::UNSUPPORTED));
    }
}
