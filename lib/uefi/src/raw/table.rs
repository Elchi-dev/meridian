//! The firmware tables of UEFI 2.10, chapter 4, field for field.

use core::ffi::c_void;

use super::console::{SimpleTextInput, SimpleTextOutput};
use super::{Event, Handle};
use crate::memory::MemoryType;
use crate::{Guid, Status};

/// A slot Meridian does not call yet. It only keeps the table layout; a
/// slot gets its real function type together with its first caller and the
/// QEMU test that exercises it.
pub type NotCalled = usize;

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

pub const SYSTEM_TABLE_SIGNATURE: u64 = u64::from_le_bytes(*b"IBI SYST");
pub const BOOT_SERVICES_SIGNATURE: u64 = u64::from_le_bytes(*b"BOOTSERV");
pub const RUNTIME_SERVICES_SIGNATURE: u64 = u64::from_le_bytes(*b"RUNTSERV");

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut ConfigurationTable,
}

#[repr(C)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *mut c_void,
}

#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: NotCalled,
    pub restore_tpl: NotCalled,
    pub allocate_pages: unsafe extern "efiapi" fn(
        kind: AllocateType,
        memory_type: MemoryType,
        pages: usize,
        memory: *mut u64,
    ) -> Status,
    pub free_pages: unsafe extern "efiapi" fn(memory: u64, pages: usize) -> Status,
    pub get_memory_map: unsafe extern "efiapi" fn(
        size: *mut usize,
        map: *mut u8,
        key: *mut usize,
        descriptor_size: *mut usize,
        descriptor_version: *mut u32,
    ) -> Status,
    pub allocate_pool: NotCalled,
    pub free_pool: NotCalled,
    pub create_event: NotCalled,
    pub set_timer: NotCalled,
    pub wait_for_event:
        unsafe extern "efiapi" fn(count: usize, events: *const Event, index: *mut usize) -> Status,
    pub signal_event: NotCalled,
    pub close_event: NotCalled,
    pub check_event: NotCalled,
    pub install_protocol_interface: NotCalled,
    pub reinstall_protocol_interface: NotCalled,
    pub uninstall_protocol_interface: NotCalled,
    pub handle_protocol: unsafe extern "efiapi" fn(
        handle: Handle,
        protocol: *const Guid,
        interface: *mut *mut c_void,
    ) -> Status,
    pub reserved: NotCalled,
    pub register_protocol_notify: NotCalled,
    pub locate_handle: NotCalled,
    pub locate_device_path: NotCalled,
    pub install_configuration_table: NotCalled,
    pub load_image: unsafe extern "efiapi" fn(
        boot_policy: bool,
        parent: Handle,
        device_path: *const u8,
        source: *const c_void,
        source_size: usize,
        image: *mut Handle,
    ) -> Status,
    pub start_image: unsafe extern "efiapi" fn(
        image: Handle,
        exit_data_size: *mut usize,
        exit_data: *mut *mut u16,
    ) -> Status,
    pub exit: NotCalled,
    pub unload_image: unsafe extern "efiapi" fn(image: Handle) -> Status,
    pub exit_boot_services: unsafe extern "efiapi" fn(image: Handle, map_key: usize) -> Status,
    pub get_next_monotonic_count: NotCalled,
    pub stall: unsafe extern "efiapi" fn(microseconds: usize) -> Status,
    pub set_watchdog_timer: NotCalled,
    pub connect_controller: NotCalled,
    pub disconnect_controller: NotCalled,
    pub open_protocol: NotCalled,
    pub close_protocol: NotCalled,
    pub open_protocol_information: NotCalled,
    pub protocols_per_handle: NotCalled,
    pub locate_handle_buffer: NotCalled,
    pub locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const Guid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> Status,
    pub install_multiple_protocol_interfaces: NotCalled,
    pub uninstall_multiple_protocol_interfaces: NotCalled,
    pub calculate_crc32: NotCalled,
    pub copy_mem: NotCalled,
    pub set_mem: NotCalled,
    pub create_event_ex: NotCalled,
}

/// `EFI_ALLOCATE_TYPE`: where `AllocatePages` may put the pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct AllocateType(pub u32);

impl AllocateType {
    pub const ANY_PAGES: AllocateType = AllocateType(0);
    pub const MAX_ADDRESS: AllocateType = AllocateType(1);
    pub const ADDRESS: AllocateType = AllocateType(2);
}

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    pub get_time: NotCalled,
    pub set_time: NotCalled,
    pub get_wakeup_time: NotCalled,
    pub set_wakeup_time: NotCalled,
    pub set_virtual_address_map: NotCalled,
    pub convert_pointer: NotCalled,
    pub get_variable: NotCalled,
    pub get_next_variable_name: NotCalled,
    pub set_variable: NotCalled,
    pub get_next_high_monotonic_count: NotCalled,
    pub reset_system: NotCalled,
    pub update_capsule: NotCalled,
    pub query_capsule_capabilities: NotCalled,
    pub query_variable_info: NotCalled,
}

/// Whether a table header carries the expected signature and claims at
/// least the size the specification gives the table.
#[must_use]
pub fn header_is_valid(hdr: &TableHeader, signature: u64, min_size: usize) -> bool {
    hdr.signature == signature && hdr.header_size as usize >= min_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn signatures_spell_their_names() {
        assert_eq!(SYSTEM_TABLE_SIGNATURE, 0x5453_5953_2049_4249);
        assert_eq!(BOOT_SERVICES_SIGNATURE, 0x5652_4553_544f_4f42);
        assert_eq!(RUNTIME_SERVICES_SIGNATURE, 0x5652_4553_544e_5552);
    }

    #[test]
    fn table_header() {
        assert_eq!(size_of::<TableHeader>(), 24);
        assert_eq!(offset_of!(TableHeader, revision), 8);
        assert_eq!(offset_of!(TableHeader, header_size), 12);
        assert_eq!(offset_of!(TableHeader, crc32), 16);
    }

    #[test]
    fn system_table() {
        assert_eq!(size_of::<SystemTable>(), 120);
        let offsets = [
            (offset_of!(SystemTable, firmware_vendor), 24),
            (offset_of!(SystemTable, firmware_revision), 32),
            (offset_of!(SystemTable, console_in_handle), 40),
            (offset_of!(SystemTable, con_in), 48),
            (offset_of!(SystemTable, console_out_handle), 56),
            (offset_of!(SystemTable, con_out), 64),
            (offset_of!(SystemTable, standard_error_handle), 72),
            (offset_of!(SystemTable, std_err), 80),
            (offset_of!(SystemTable, runtime_services), 88),
            (offset_of!(SystemTable, boot_services), 96),
            (offset_of!(SystemTable, number_of_table_entries), 104),
            (offset_of!(SystemTable, configuration_table), 112),
        ];
        for (i, (got, want)) in offsets.into_iter().enumerate() {
            assert_eq!(got, want, "field {i}");
        }
        assert_eq!(size_of::<ConfigurationTable>(), 24);
        assert_eq!(offset_of!(ConfigurationTable, vendor_guid), 0);
        assert_eq!(offset_of!(ConfigurationTable, vendor_table), 16);
    }

    #[test]
    fn boot_services_slots() {
        let offsets = [
            offset_of!(BootServices, raise_tpl),
            offset_of!(BootServices, restore_tpl),
            offset_of!(BootServices, allocate_pages),
            offset_of!(BootServices, free_pages),
            offset_of!(BootServices, get_memory_map),
            offset_of!(BootServices, allocate_pool),
            offset_of!(BootServices, free_pool),
            offset_of!(BootServices, create_event),
            offset_of!(BootServices, set_timer),
            offset_of!(BootServices, wait_for_event),
            offset_of!(BootServices, signal_event),
            offset_of!(BootServices, close_event),
            offset_of!(BootServices, check_event),
            offset_of!(BootServices, install_protocol_interface),
            offset_of!(BootServices, reinstall_protocol_interface),
            offset_of!(BootServices, uninstall_protocol_interface),
            offset_of!(BootServices, handle_protocol),
            offset_of!(BootServices, reserved),
            offset_of!(BootServices, register_protocol_notify),
            offset_of!(BootServices, locate_handle),
            offset_of!(BootServices, locate_device_path),
            offset_of!(BootServices, install_configuration_table),
            offset_of!(BootServices, load_image),
            offset_of!(BootServices, start_image),
            offset_of!(BootServices, exit),
            offset_of!(BootServices, unload_image),
            offset_of!(BootServices, exit_boot_services),
            offset_of!(BootServices, get_next_monotonic_count),
            offset_of!(BootServices, stall),
            offset_of!(BootServices, set_watchdog_timer),
            offset_of!(BootServices, connect_controller),
            offset_of!(BootServices, disconnect_controller),
            offset_of!(BootServices, open_protocol),
            offset_of!(BootServices, close_protocol),
            offset_of!(BootServices, open_protocol_information),
            offset_of!(BootServices, protocols_per_handle),
            offset_of!(BootServices, locate_handle_buffer),
            offset_of!(BootServices, locate_protocol),
            offset_of!(BootServices, install_multiple_protocol_interfaces),
            offset_of!(BootServices, uninstall_multiple_protocol_interfaces),
            offset_of!(BootServices, calculate_crc32),
            offset_of!(BootServices, copy_mem),
            offset_of!(BootServices, set_mem),
            offset_of!(BootServices, create_event_ex),
        ];
        for (slot, offset) in offsets.into_iter().enumerate() {
            assert_eq!(offset, 24 + 8 * slot, "slot {slot}");
        }
        assert_eq!(size_of::<BootServices>(), 24 + 8 * 44);
        assert_eq!(offset_of!(BootServices, exit_boot_services), 0xe8);
        assert_eq!(offset_of!(BootServices, locate_protocol), 0x140);
    }

    #[test]
    fn allocate_types() {
        assert_eq!(size_of::<AllocateType>(), 4);
        assert_eq!(AllocateType::ANY_PAGES.0, 0);
        assert_eq!(AllocateType::MAX_ADDRESS.0, 1);
        assert_eq!(AllocateType::ADDRESS.0, 2);
    }

    #[test]
    fn runtime_services_slots() {
        let offsets = [
            offset_of!(RuntimeServices, get_time),
            offset_of!(RuntimeServices, set_time),
            offset_of!(RuntimeServices, get_wakeup_time),
            offset_of!(RuntimeServices, set_wakeup_time),
            offset_of!(RuntimeServices, set_virtual_address_map),
            offset_of!(RuntimeServices, convert_pointer),
            offset_of!(RuntimeServices, get_variable),
            offset_of!(RuntimeServices, get_next_variable_name),
            offset_of!(RuntimeServices, set_variable),
            offset_of!(RuntimeServices, get_next_high_monotonic_count),
            offset_of!(RuntimeServices, reset_system),
            offset_of!(RuntimeServices, update_capsule),
            offset_of!(RuntimeServices, query_capsule_capabilities),
            offset_of!(RuntimeServices, query_variable_info),
        ];
        for (slot, offset) in offsets.into_iter().enumerate() {
            assert_eq!(offset, 24 + 8 * slot, "slot {slot}");
        }
        assert_eq!(size_of::<RuntimeServices>(), 24 + 8 * 14);
    }

    fn header(signature: u64, size: u32) -> TableHeader {
        TableHeader {
            signature,
            revision: 0x0002_000a,
            header_size: size,
            crc32: 0,
            reserved: 0,
        }
    }

    #[test]
    fn header_validation() {
        assert!(header_is_valid(
            &header(SYSTEM_TABLE_SIGNATURE, 120),
            SYSTEM_TABLE_SIGNATURE,
            120
        ));
        assert!(header_is_valid(
            &header(SYSTEM_TABLE_SIGNATURE, 128),
            SYSTEM_TABLE_SIGNATURE,
            120
        ));
        assert!(!header_is_valid(
            &header(SYSTEM_TABLE_SIGNATURE, 119),
            SYSTEM_TABLE_SIGNATURE,
            120
        ));
        assert!(!header_is_valid(
            &header(BOOT_SERVICES_SIGNATURE, 120),
            SYSTEM_TABLE_SIGNATURE,
            120
        ));
    }
}
