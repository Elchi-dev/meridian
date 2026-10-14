//! Loading and starting another EFI application from the partition this
//! one came from, which is how the boot screen starts other systems
//! (UEFI 2.10, section 7.4).

use core::ffi::c_void;
use core::ptr::{self, NonNull};

use super::file::LoadedImage;
use super::{Handle, System};
use crate::Status;
use crate::device_path;
use crate::guid::{DEVICE_PATH_PROTOCOL, LOADED_IMAGE_PROTOCOL, SIMPLE_FILE_SYSTEM_PROTOCOL};

/// The longest device path the loader builds, in bytes.
pub const PATH_BYTES: usize = 1024;

/// How many file systems [`System::file_system_paths`] can list.
const FILE_SYSTEMS: usize = 32;

/// `LocateHandle`'s search for every handle with a given protocol.
const BY_PROTOCOL: u32 = 2;

impl System {
    fn loaded_image(&self, handle: Handle) -> Result<NonNull<LoadedImage>, Status> {
        self.protocol::<LoadedImage>(handle, &LOADED_IMAGE_PROTOCOL)
    }

    /// The options this image was started with, as UCS-2 units.
    #[must_use]
    pub fn load_options(&self) -> &[u16] {
        let Ok(image) = self.loaded_image(self.image()) else {
            return &[];
        };
        // SAFETY: the firmware's own loaded image protocol for this image,
        // which lives as long as the image does.
        let image = unsafe { image.as_ref() };
        let units = image.load_options_size as usize / 2;
        if image.load_options.is_null() {
            return &[];
        }
        // SAFETY: the firmware, or whoever started this image, put this
        // many bytes of options there, and they stay for as long as the
        // image runs.
        unsafe { core::slice::from_raw_parts(image.load_options.cast::<u16>(), units) }
    }

    /// The device path of the partition this image was loaded from,
    /// copied into `out` with its end node.
    ///
    /// # Errors
    ///
    /// The firmware's status, `BUFFER_TOO_SMALL` for a path longer than
    /// `out`, and `VOLUME_CORRUPTED` for a node shorter than its header.
    pub fn boot_device_path<'b>(&self, out: &'b mut [u8]) -> Result<&'b [u8], Status> {
        // SAFETY: the firmware's own loaded image protocol for this image.
        let device = unsafe { self.loaded_image(self.image())?.as_ref().device_handle };
        self.device_path_of(device, out)
    }

    /// Calls `each` with the device path of every partition or disk the
    /// firmware can read files from, the one this image came from among
    /// them, and returns how many there are. Paths longer than
    /// [`PATH_BYTES`] are left out.
    ///
    /// # Errors
    ///
    /// The firmware's status, and `BUFFER_TOO_SMALL` for a machine with
    /// more than 32 of them.
    pub fn file_system_paths(&self, mut each: impl FnMut(&[u8])) -> Result<usize, Status> {
        let mut handles: [Handle; FILE_SYSTEMS] = [ptr::null_mut(); FILE_SYSTEMS];
        let mut size = size_of_val(&handles);
        // SAFETY: LocateHandle writes at most size bytes of handles into
        // the array and sets size to how many bytes it wrote.
        unsafe {
            (self.boot_services().locate_handle)(
                BY_PROTOCOL,
                &SIMPLE_FILE_SYSTEM_PROTOCOL,
                ptr::null(),
                &raw mut size,
                handles.as_mut_ptr(),
            )
        }
        .ok()?;
        let mut path = [0u8; PATH_BYTES];
        let found = (size / size_of::<Handle>()).min(FILE_SYSTEMS);
        for &handle in &handles[..found] {
            if let Ok(p) = self.device_path_of(handle, &mut path) {
                each(p);
            }
        }
        Ok(found)
    }

    /// The device path of `handle`, copied into `out` with its end node.
    fn device_path_of<'b>(&self, handle: Handle, out: &'b mut [u8]) -> Result<&'b [u8], Status> {
        let path = self.protocol::<u8>(handle, &DEVICE_PATH_PROTOCOL)?.as_ptr();
        let mut len = 0;
        loop {
            // SAFETY: a device path is a chain of nodes, each starting with
            // a four byte header that holds its length; the walk reads one
            // header at a time and stops at the end node.
            let header = unsafe { ptr::read_unaligned(path.add(len).cast::<[u8; 4]>()) };
            let node = usize::from(u16::from_le_bytes([header[2], header[3]]));
            if node < 4 {
                return Err(Status::VOLUME_CORRUPTED);
            }
            if len + node > out.len() {
                return Err(Status::BUFFER_TOO_SMALL);
            }
            // SAFETY: the node's header says it is this long.
            let bytes = unsafe { core::slice::from_raw_parts(path.add(len), node) };
            out[len..len + node].copy_from_slice(bytes);
            len += node;
            if device_path::is_end(header) {
                return Ok(&out[..len]);
            }
        }
    }

    /// Loads the EFI application at `file` on the partition this image
    /// came from. The firmware checks its signature when Secure Boot is on.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` for a file that is not there, `SECURITY_VIOLATION` for
    /// one Secure Boot refuses, and the firmware's other statuses.
    pub fn load_image(&self, file: &str) -> Result<Image<'_>, Status> {
        let mut base = [0u8; PATH_BYTES];
        let base = self.boot_device_path(&mut base)?;
        let mut path = [0u8; PATH_BYTES];
        let len = device_path::with_file(base, file, &mut path).ok_or(Status::INVALID_PARAMETER)?;
        self.load_image_at(&path[..len])
    }

    /// Loads the EFI application a whole device path names, such as the
    /// path of a firmware boot option.
    ///
    /// # Errors
    ///
    /// `INVALID_PARAMETER` for a path that is broken or has no end node,
    /// and the statuses of [`System::load_image`].
    pub fn load_image_at(&self, path: &[u8]) -> Result<Image<'_>, Status> {
        if device_path::nodes(path).is_none() {
            return Err(Status::INVALID_PARAMETER);
        }
        let mut handle = ptr::null_mut();
        // SAFETY: the path is whole and ends in an end node; with no
        // source buffer the firmware reads the file itself.
        unsafe {
            (self.boot_services().load_image)(
                false,
                self.image(),
                path.as_ptr(),
                ptr::null(),
                0,
                &raw mut handle,
            )
        }
        .ok()?;
        Ok(Image {
            handle,
            system: self,
            started: false,
        })
    }
}

/// An application that is loaded and not started yet. Dropping it
/// unloads it again.
pub struct Image<'a> {
    handle: Handle,
    system: &'a System,
    started: bool,
}

impl<'a> Image<'a> {
    /// Hands the image `options` to read when it starts, such as a
    /// kernel command line.
    ///
    /// # Errors
    ///
    /// The firmware's status when the image has no loaded image protocol.
    pub fn set_options(&mut self, options: &'a [u16]) -> Result<(), Status> {
        let size = u32::try_from(options.len() * 2).map_err(|_| Status::INVALID_PARAMETER)?;
        let image = self.system.loaded_image(self.handle)?.as_ptr();
        // SAFETY: the loaded image protocol of an image this value loaded
        // and has not started, which only the image itself reads, once it
        // runs; the options outlive this value.
        unsafe {
            (*image).load_options = options.as_ptr().cast::<c_void>().cast_mut();
            (*image).load_options_size = size;
        }
        Ok(())
    }

    /// The options the image will see, read back from its loaded image
    /// protocol. Only the QEMU checks look.
    #[cfg(feature = "qemu-test")]
    #[must_use]
    pub fn options(&self) -> Vec16<'_> {
        let Ok(image) = self.system.loaded_image(self.handle) else {
            return Vec16(&[]);
        };
        // SAFETY: the loaded image protocol of an image this value loaded;
        // the options were set by set_options and outlive this value.
        let image = unsafe { image.as_ref() };
        if image.load_options.is_null() {
            return Vec16(&[]);
        }
        let units = image.load_options_size as usize / 2;
        // SAFETY: as above, that many units were set.
        Vec16(unsafe { core::slice::from_raw_parts(image.load_options.cast::<u16>(), units) })
    }

    /// The image's handle, which the QEMU checks use to see that dropping
    /// the image unloads it.
    #[cfg(feature = "qemu-test")]
    #[must_use]
    pub fn handle(&self) -> Handle {
        self.handle
    }

    /// Starts the image. Returns its exit status if it comes back, as an
    /// application does once it is done; the firmware then unloads it.
    #[must_use]
    pub fn start(mut self) -> Status {
        self.started = true;
        // SAFETY: a loaded image, started once; no exit data is asked for.
        unsafe {
            (self.system.boot_services().start_image)(self.handle, ptr::null_mut(), ptr::null_mut())
        }
    }
}

impl Drop for Image<'_> {
    fn drop(&mut self) {
        if !self.started {
            // SAFETY: the image was loaded and never started.
            let _ = unsafe { (self.system.boot_services().unload_image)(self.handle) };
        }
    }
}

#[cfg(feature = "qemu-test")]
impl System {
    /// Whether an image with this handle is still loaded.
    #[must_use]
    pub fn is_loaded(&self, handle: Handle) -> bool {
        self.loaded_image(handle).is_ok()
    }
}

/// UCS-2 units read back from an image, for the QEMU checks.
#[cfg(feature = "qemu-test")]
#[derive(Debug, PartialEq, Eq)]
pub struct Vec16<'a>(pub &'a [u16]);
