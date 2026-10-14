//! `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL` and `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`
//! (UEFI 2.10, sections 12.3 and 12.4).

use super::Event;
use super::table::NotCalled;
use crate::Status;
use crate::text::InputKey;

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: NotCalled,
    pub output_string:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, string: *const u16) -> Status,
    pub test_string: NotCalled,
    pub query_mode: NotCalled,
    pub set_mode: NotCalled,
    pub set_attribute: NotCalled,
    pub clear_screen: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput) -> Status,
    pub set_cursor_position: NotCalled,
    pub enable_cursor: NotCalled,
    pub mode: *mut core::ffi::c_void,
}

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: NotCalled,
    pub read_key_stroke:
        unsafe extern "efiapi" fn(this: *mut SimpleTextInput, key: *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn output_protocol_slots() {
        let offsets = [
            offset_of!(SimpleTextOutput, reset),
            offset_of!(SimpleTextOutput, output_string),
            offset_of!(SimpleTextOutput, test_string),
            offset_of!(SimpleTextOutput, query_mode),
            offset_of!(SimpleTextOutput, set_mode),
            offset_of!(SimpleTextOutput, set_attribute),
            offset_of!(SimpleTextOutput, clear_screen),
            offset_of!(SimpleTextOutput, set_cursor_position),
            offset_of!(SimpleTextOutput, enable_cursor),
            offset_of!(SimpleTextOutput, mode),
        ];
        for (slot, offset) in offsets.into_iter().enumerate() {
            assert_eq!(offset, 8 * slot, "slot {slot}");
        }
        assert_eq!(size_of::<SimpleTextOutput>(), 80);
    }

    #[test]
    fn input_protocol_slots() {
        assert_eq!(offset_of!(SimpleTextInput, reset), 0);
        assert_eq!(offset_of!(SimpleTextInput, read_key_stroke), 8);
        assert_eq!(offset_of!(SimpleTextInput, wait_for_key), 16);
        assert_eq!(size_of::<SimpleTextInput>(), 24);
    }
}
