//! Reading files from the volume the image was started from: the loaded
//! image protocol finds the volume, the simple file system protocol opens
//! it, and file protocol instances read it (UEFI 2.10, sections 9.1, 13.4
//! and 13.5).

use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::table::{NotCalled, SystemTable};
use super::{Handle, System};
use crate::guid::{FILE_INFO, LOADED_IMAGE_PROTOCOL, SIMPLE_FILE_SYSTEM_PROTOCOL};
use crate::memory::MemoryType;
use crate::text::file_path;
use crate::{Guid, Status};

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    pub file_path: *mut c_void,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: MemoryType,
    pub image_data_type: MemoryType,
    pub unload: NotCalled,
}

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(
        this: *mut SimpleFileSystem,
        root: *mut *mut FileProtocol,
    ) -> Status,
}

#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new: *mut *mut FileProtocol,
        name: *const u16,
        mode: u64,
        attributes: u64,
    ) -> Status,
    pub close: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    pub delete: NotCalled,
    pub read: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        size: *mut usize,
        buffer: *mut u8,
    ) -> Status,
    pub write: NotCalled,
    pub get_position: NotCalled,
    pub set_position: NotCalled,
    pub get_info: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        info: *const Guid,
        size: *mut usize,
        buffer: *mut u8,
    ) -> Status,
    pub set_info: NotCalled,
    pub flush: NotCalled,
}

pub const MODE_READ: u64 = 1;

/// `EFI_FILE_INFO` is 80 bytes of fixed fields and then the file name.
const INFO_NAME_AT: usize = 80;
const ATTRIBUTE_DIRECTORY: u64 = 0x10;

/// The longest path [`File::open`] takes, in UCS-2 units.
pub const PATH_LIMIT: usize = 255;

/// What `EFI_FILE_INFO` says about a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub size: u64,
    pub directory: bool,
}

/// One entry of a directory: what `EFI_FILE_INFO` says, and the name,
/// read from the buffer it was read into.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'b> {
    pub info: Info,
    name: &'b [u8],
}

impl Entry<'_> {
    /// The entry's name, as the file system stores it.
    pub fn name(&self) -> impl Iterator<Item = char> + '_ {
        let units = self
            .name
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b));
        char::decode_utf16(units.take_while(|&u| u != 0))
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// Whether the name is `other`, ignoring ASCII case as FAT does.
    #[must_use]
    pub fn is_named(&self, other: &str) -> bool {
        let mut mine = self.name();
        let mut theirs = other.chars();
        loop {
            match (mine.next(), theirs.next()) {
                (None, None) => return true,
                (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b) => {}
                _ => return false,
            }
        }
    }

    /// Reads an entry from the bytes `EFI_FILE_INFO` takes.
    fn read(b: &[u8]) -> Option<Entry<'_>> {
        Some(Entry {
            info: Info::read(b)?,
            name: &b[INFO_NAME_AT..],
        })
    }
}

/// Room for one directory entry with a name up to [`PATH_LIMIT`] units.
pub type EntryBuffer = [u8; INFO_NAME_AT + 2 * (PATH_LIMIT + 1)];

impl Info {
    fn read(b: &[u8]) -> Option<Info> {
        if b.len() < INFO_NAME_AT {
            return None;
        }
        let u64_at = |at: usize| {
            let mut v = [0u8; 8];
            v.copy_from_slice(&b[at..at + 8]);
            u64::from_le_bytes(v)
        };
        Some(Info {
            size: u64_at(8),
            directory: u64_at(72) & ATTRIBUTE_DIRECTORY != 0,
        })
    }
}

impl System {
    pub(super) fn protocol<T>(&self, handle: Handle, guid: &Guid) -> Result<NonNull<T>, Status> {
        let mut interface = ptr::null_mut();
        // SAFETY: HandleProtocol writes one interface pointer.
        unsafe { (self.boot_services().handle_protocol)(handle, guid, &raw mut interface) }.ok()?;
        NonNull::new(interface.cast()).ok_or(Status::UNSUPPORTED)
    }

    /// Where this image lies in memory, as its first address and its size
    /// in bytes.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status if it has no loaded image protocol for
    /// the image.
    pub fn image_range(&self) -> Result<(u64, u64), Status> {
        let image = self.protocol::<LoadedImage>(self.image(), &LOADED_IMAGE_PROTOCOL)?;
        // SAFETY: the firmware's own loaded image protocol for this image.
        let image = unsafe { image.as_ref() };
        Ok((image.image_base as u64, image.image_size))
    }

    /// The root directory of the volume this image was loaded from.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status when the image did not come from a
    /// file system it can read.
    pub fn boot_volume(&self) -> Result<File<'_>, Status> {
        let image = self.protocol::<LoadedImage>(self.image(), &LOADED_IMAGE_PROTOCOL)?;
        // SAFETY: the firmware's own loaded image protocol for this image.
        let device = unsafe { image.as_ref().device_handle };
        let fs = self.protocol::<SimpleFileSystem>(device, &SIMPLE_FILE_SYSTEM_PROTOCOL)?;
        let mut root = ptr::null_mut();
        // SAFETY: fs is the device's file system protocol; OpenVolume
        // writes one file protocol pointer.
        unsafe { (fs.as_ref().open_volume)(fs.as_ptr(), &raw mut root) }.ok()?;
        let handle = NonNull::new(root).ok_or(Status::UNSUPPORTED)?;
        Ok(File {
            handle,
            system: self,
        })
    }
}

/// An open file or directory, closed when dropped.
pub struct File<'a> {
    handle: NonNull<FileProtocol>,
    system: &'a System,
}

impl<'a> File<'a> {
    fn protocol(&self) -> &FileProtocol {
        // SAFETY: the handle came from the firmware and stays valid until
        // Close, which only Drop calls.
        unsafe { self.handle.as_ref() }
    }

    /// Opens `path` for reading, relative to this directory unless it
    /// starts with a separator. Both `/` and `\` separate directories.
    ///
    /// # Errors
    ///
    /// `INVALID_PARAMETER` for a path longer than [`PATH_LIMIT`] or with a
    /// character UCS-2 cannot hold, and the firmware's status otherwise,
    /// `NOT_FOUND` above all.
    pub fn open(&self, path: &str) -> Result<File<'a>, Status> {
        let mut name = [0u16; PATH_LIMIT + 1];
        file_path(path, &mut name).ok_or(Status::INVALID_PARAMETER)?;
        let mut new = ptr::null_mut();
        // SAFETY: name is NUL-terminated and Open writes one pointer.
        unsafe {
            (self.protocol().open)(
                self.handle.as_ptr(),
                &raw mut new,
                name.as_ptr(),
                MODE_READ,
                0,
            )
        }
        .ok()?;
        let handle = NonNull::new(new).ok_or(Status::NOT_FOUND)?;
        Ok(File {
            handle,
            system: self.system,
        })
    }

    /// The file's size and whether it is a directory.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status, `BUFFER_TOO_SMALL` for a name longer
    /// than [`PATH_LIMIT`].
    pub fn info(&self) -> Result<Info, Status> {
        let mut buf = [0u8; INFO_NAME_AT + 2 * (PATH_LIMIT + 1)];
        let mut size = buf.len();
        // SAFETY: GetInfo writes at most size bytes into buf.
        unsafe {
            (self.protocol().get_info)(
                self.handle.as_ptr(),
                &FILE_INFO,
                &raw mut size,
                buf.as_mut_ptr(),
            )
        }
        .ok()?;
        Info::read(&buf[..size.min(buf.len())]).ok_or(Status::VOLUME_CORRUPTED)
    }

    /// The next entry of this directory, read into `buf`, or `None` after
    /// the last. The entries "." and ".." come too.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status, `BUFFER_TOO_SMALL` for a name longer
    /// than [`PATH_LIMIT`].
    pub fn next_entry<'b>(
        &mut self,
        buf: &'b mut EntryBuffer,
    ) -> Result<Option<Entry<'b>>, Status> {
        let got = self.read(buf)?;
        if got == 0 {
            return Ok(None);
        }
        Entry::read(&buf[..got])
            .map(Some)
            .ok_or(Status::VOLUME_CORRUPTED)
    }

    /// Reads from the current position into `buf` and returns how many
    /// bytes came; 0 means the end of the file.
    ///
    /// # Errors
    ///
    /// Returns the firmware's status, `DEVICE_ERROR` above all.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Status> {
        let mut size = buf.len();
        // SAFETY: Read writes at most size bytes into buf.
        unsafe { (self.protocol().read)(self.handle.as_ptr(), &raw mut size, buf.as_mut_ptr()) }
            .ok()?;
        Ok(size.min(buf.len()))
    }

    /// Reads until `buf` is full or the file ends, and returns how many
    /// bytes came.
    ///
    /// # Errors
    ///
    /// Like [`File::read`].
    pub fn read_all(&mut self, buf: &mut [u8]) -> Result<usize, Status> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.read(&mut buf[done..])?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }
}

impl Drop for File<'_> {
    fn drop(&mut self) {
        // SAFETY: closes the handle exactly once; nothing uses it after.
        let _ = unsafe { (self.protocol().close)(self.handle.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn loaded_image_layout() {
        let offsets = [
            (offset_of!(LoadedImage, parent_handle), 8),
            (offset_of!(LoadedImage, system_table), 16),
            (offset_of!(LoadedImage, device_handle), 24),
            (offset_of!(LoadedImage, file_path), 32),
            (offset_of!(LoadedImage, reserved), 40),
            (offset_of!(LoadedImage, load_options_size), 48),
            (offset_of!(LoadedImage, load_options), 56),
            (offset_of!(LoadedImage, image_base), 64),
            (offset_of!(LoadedImage, image_size), 72),
            (offset_of!(LoadedImage, image_code_type), 80),
            (offset_of!(LoadedImage, image_data_type), 84),
            (offset_of!(LoadedImage, unload), 88),
        ];
        for (got, want) in offsets {
            assert_eq!(got, want);
        }
        assert_eq!(size_of::<LoadedImage>(), 96);
    }

    #[test]
    fn file_system_layout() {
        assert_eq!(offset_of!(SimpleFileSystem, open_volume), 8);
        assert_eq!(size_of::<SimpleFileSystem>(), 16);
    }

    #[test]
    fn file_protocol_slots() {
        let offsets = [
            offset_of!(FileProtocol, open),
            offset_of!(FileProtocol, close),
            offset_of!(FileProtocol, delete),
            offset_of!(FileProtocol, read),
            offset_of!(FileProtocol, write),
            offset_of!(FileProtocol, get_position),
            offset_of!(FileProtocol, set_position),
            offset_of!(FileProtocol, get_info),
            offset_of!(FileProtocol, set_info),
            offset_of!(FileProtocol, flush),
        ];
        for (slot, offset) in offsets.into_iter().enumerate() {
            assert_eq!(offset, 8 + 8 * slot, "slot {slot}");
        }
        assert_eq!(size_of::<FileProtocol>(), 88);
    }

    fn info_bytes(size: u64, attribute: u64, name: &str) -> Vec<u8> {
        let mut b = vec![0u8; INFO_NAME_AT];
        b[8..16].copy_from_slice(&size.to_le_bytes());
        b[16..24].copy_from_slice(&(size.saturating_add(4095) & !4095).to_le_bytes());
        b[24..72].fill(0x5a);
        b[72..80].copy_from_slice(&attribute.to_le_bytes());
        for unit in name.encode_utf16().chain([0]) {
            b.extend(unit.to_le_bytes());
        }
        let total = b.len() as u64;
        b[0..8].copy_from_slice(&total.to_le_bytes());
        b
    }

    #[test]
    fn info_reads_size_and_directory_bit() {
        let file = info_bytes(0x1_2345, 0x20, "KERNEL");
        assert_eq!(
            Info::read(&file),
            Some(Info {
                size: 0x1_2345,
                directory: false
            })
        );
        let dir = info_bytes(0, 0x10 | 0x02, "EFI");
        assert_eq!(
            Info::read(&dir),
            Some(Info {
                size: 0,
                directory: true
            })
        );
        let big = info_bytes(u64::MAX - 5, 0x01, "");
        assert_eq!(Info::read(&big).map(|i| i.size), Some(u64::MAX - 5));
    }

    #[test]
    fn entries_carry_their_names() {
        let bytes = info_bytes(0, 0x10, "Microsoft");
        let entry = Entry::read(&bytes).unwrap();
        assert!(entry.info.directory);
        assert_eq!(entry.name().collect::<String>(), "Microsoft");
        assert!(entry.is_named("microsoft") && entry.is_named("MICROSOFT"));
        assert!(!entry.is_named("Micro") && !entry.is_named("Microsofts") && !entry.is_named(""));
        let file = info_bytes(42, 0x20, "grubx64.efi");
        let entry = Entry::read(&file).unwrap();
        assert_eq!((entry.info.size, entry.info.directory), (42, false));
        assert!(entry.is_named("GRUBX64.EFI"));
        assert_eq!(Entry::read(&file[..INFO_NAME_AT - 1]).map(|e| e.info), None);
    }

    #[test]
    fn only_ascii_letters_ignore_case() {
        let bytes = info_bytes(0, 0, "\u{c4}rger");
        let entry = Entry::read(&bytes).unwrap();
        assert!(entry.is_named("\u{c4}RGER"), "ASCII letters ignore case");
        assert!(
            !entry.is_named("\u{e4}rger"),
            "others do not, as FAT's own table is not known here"
        );
    }

    #[test]
    fn names_stop_at_their_terminator_and_survive_odd_units() {
        let mut bytes = info_bytes(0, 0, "ab");
        bytes.extend_from_slice(&[0x63, 0, 0, 0]);
        assert_eq!(
            Entry::read(&bytes).unwrap().name().collect::<String>(),
            "ab",
            "nothing after the NUL"
        );
        let mut lone = info_bytes(0, 0, "");
        lone.truncate(INFO_NAME_AT);
        lone.extend_from_slice(&[0x00, 0xd8, 0x41, 0x00, 0, 0]);
        assert_eq!(
            Entry::read(&lone).unwrap().name().collect::<String>(),
            "\u{fffd}A",
            "a lone surrogate"
        );
        let empty = info_bytes(0, 0, "");
        assert!(Entry::read(&empty).unwrap().is_named(""));
        assert_eq!(size_of::<EntryBuffer>(), INFO_NAME_AT + 512);
    }

    #[test]
    fn info_needs_the_fixed_fields() {
        let file = info_bytes(7, 0, "");
        assert_eq!(
            Info::read(&file[..INFO_NAME_AT]),
            Some(Info {
                size: 7,
                directory: false
            })
        );
        assert_eq!(Info::read(&file[..INFO_NAME_AT - 1]), None);
        assert_eq!(Info::read(&[]), None);
    }
}
