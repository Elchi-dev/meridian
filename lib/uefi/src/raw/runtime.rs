//! The runtime services the loader calls while boot services still run:
//! firmware variables and restarting the machine.

use core::ffi::c_void;

use super::System;
use super::table::{RUNTIME_SERVICES_SIGNATURE, ResetType, RuntimeServices, header_is_valid};
use crate::variable::{Attributes, is_name};
use crate::{Guid, Status};

impl System {
    /// The runtime services table, once its header checks out.
    fn runtime_services(&self) -> Result<&RuntimeServices, Status> {
        let rt = self.table().runtime_services;
        if rt.is_null() {
            return Err(Status::UNSUPPORTED);
        }
        // SAFETY: the system table points at the firmware's runtime
        // services. Only the header is read until it has been checked.
        let hdr = unsafe { &(*rt).hdr };
        if !header_is_valid(
            hdr,
            RUNTIME_SERVICES_SIGNATURE,
            size_of::<RuntimeServices>(),
        ) {
            return Err(Status::UNSUPPORTED);
        }
        // SAFETY: checked above; the firmware keeps the table for as long
        // as it runs.
        Ok(unsafe { &*rt })
    }

    /// Reads the variable `name` of `vendor` into `buf`, and returns its
    /// attributes and its data, which starts `buf`.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` when there is no such variable, `BUFFER_TOO_SMALL` when
    /// it does not fit, `INVALID_PARAMETER` for a name without its single
    /// closing 0, and the firmware's other errors.
    pub fn variable<'b>(
        &self,
        name: &[u16],
        vendor: &Guid,
        buf: &'b mut [u8],
    ) -> Result<(Attributes, &'b [u8]), Status> {
        if !is_name(name) {
            return Err(Status::INVALID_PARAMETER);
        }
        let rt = self.runtime_services()?;
        let mut attributes = 0;
        let mut size = buf.len();
        // SAFETY: the name ends in its 0, and the firmware writes at most
        // size bytes into buf and the size and attributes into their places.
        unsafe {
            (rt.get_variable)(
                name.as_ptr(),
                vendor,
                &raw mut attributes,
                &raw mut size,
                buf.as_mut_ptr().cast::<c_void>(),
            )
        }
        .ok()?;
        let data = buf.get(..size).ok_or(Status::BAD_BUFFER_SIZE)?;
        Ok((Attributes(attributes), data))
    }

    /// Sets the variable `name` of `vendor` to `data`. Empty data with no
    /// attributes deletes it.
    ///
    /// # Errors
    ///
    /// `INVALID_PARAMETER` for a name without its single closing 0, and the
    /// firmware's errors, such as `WRITE_PROTECTED` for a variable only the
    /// firmware may change.
    pub fn set_variable(
        &self,
        name: &[u16],
        vendor: &Guid,
        attributes: Attributes,
        data: &[u8],
    ) -> Result<(), Status> {
        if !is_name(name) {
            return Err(Status::INVALID_PARAMETER);
        }
        let rt = self.runtime_services()?;
        // SAFETY: the name ends in its 0 and the firmware only reads
        // data.len() bytes of data.
        unsafe {
            (rt.set_variable)(
                name.as_ptr(),
                vendor,
                attributes.0,
                data.len(),
                data.as_ptr().cast::<c_void>(),
            )
        }
        .ok()
    }

    /// Restarts the machine, or switches it off. Returns only when the
    /// firmware has no runtime services to do it with, and says so.
    pub fn reset(&self, kind: ResetType) -> Status {
        let Ok(rt) = self.runtime_services() else {
            return Status::UNSUPPORTED;
        };
        // SAFETY: a reset with no data.
        unsafe { (rt.reset_system)(kind, Status::SUCCESS, 0, core::ptr::null()) };
        // The firmware does not come back. If it did, waiting here is safer
        // than going on in a state nobody planned for.
        loop {
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::table::{
        BOOT_SERVICES_SIGNATURE, BootServices, SYSTEM_TABLE_SIGNATURE, SystemTable, TableHeader,
    };
    use crate::variable::OS_INDICATIONS;
    use core::mem::MaybeUninit;

    fn header(signature: u64, size: usize) -> TableHeader {
        TableHeader {
            signature,
            revision: (2 << 16) + 70,
            header_size: u32::try_from(size).unwrap(),
            crc32: 0,
            reserved: 0,
        }
    }

    /// Tables with only their headers filled in. Nothing here calls into
    /// them: every test stops at a check before the firmware would be
    /// called.
    struct Tables {
        bs: MaybeUninit<BootServices>,
        rt: MaybeUninit<RuntimeServices>,
        st: MaybeUninit<SystemTable>,
    }

    impl Tables {
        fn new(runtime_signature: u64, runtime_size: usize, with_runtime: bool) -> Box<Tables> {
            let mut t = Box::new(Tables {
                bs: MaybeUninit::zeroed(),
                rt: MaybeUninit::zeroed(),
                st: MaybeUninit::zeroed(),
            });
            let bs = t.bs.as_mut_ptr();
            let rt = t.rt.as_mut_ptr();
            let st = t.st.as_mut_ptr();
            // SAFETY: only the headers and pointers of memory owned here
            // are written.
            unsafe {
                (&raw mut (*bs).hdr)
                    .write(header(BOOT_SERVICES_SIGNATURE, size_of::<BootServices>()));
                (&raw mut (*rt).hdr).write(header(runtime_signature, runtime_size));
                (&raw mut (*st).hdr)
                    .write(header(SYSTEM_TABLE_SIGNATURE, size_of::<SystemTable>()));
                (&raw mut (*st).boot_services).write(bs);
                (&raw mut (*st).runtime_services).write(if with_runtime {
                    rt
                } else {
                    core::ptr::null_mut()
                });
            }
            t
        }

        fn system(&mut self) -> System {
            // SAFETY: the tables outlive the System in every test.
            unsafe { System::from_entry(core::ptr::null_mut(), self.st.as_mut_ptr()) }.unwrap()
        }
    }

    const RT_SIZE: usize = size_of::<RuntimeServices>();

    #[test]
    fn no_runtime_services_means_unsupported() {
        let mut t = Tables::new(RUNTIME_SERVICES_SIGNATURE, RT_SIZE, false);
        let system = t.system();
        let mut buf = [0u8; 8];
        let vendor = crate::guid::GLOBAL_VARIABLE;
        assert_eq!(
            system.variable(&OS_INDICATIONS, &vendor, &mut buf).err(),
            Some(Status::UNSUPPORTED)
        );
        assert_eq!(
            system.set_variable(&OS_INDICATIONS, &vendor, Attributes::default(), &[]),
            Err(Status::UNSUPPORTED)
        );
        assert_eq!(system.reset(ResetType::COLD), Status::UNSUPPORTED);
    }

    #[test]
    fn a_wrong_runtime_table_is_not_called() {
        let vendor = crate::guid::GLOBAL_VARIABLE;
        for (signature, size) in [
            (BOOT_SERVICES_SIGNATURE, RT_SIZE),
            (RUNTIME_SERVICES_SIGNATURE, RT_SIZE - 1),
        ] {
            let mut t = Tables::new(signature, size, true);
            let system = t.system();
            let mut buf = [0u8; 8];
            assert_eq!(
                system.variable(&OS_INDICATIONS, &vendor, &mut buf).err(),
                Some(Status::UNSUPPORTED),
                "{signature:#x}, {size}"
            );
            assert_eq!(system.reset(ResetType::WARM), Status::UNSUPPORTED);
        }
    }

    #[test]
    fn names_are_checked_before_the_firmware_is_asked() {
        // A valid runtime table whose slots are empty: reaching them would
        // crash the test, so the refusal has to come first.
        let mut t = Tables::new(RUNTIME_SERVICES_SIGNATURE, RT_SIZE, true);
        let system = t.system();
        let vendor = crate::guid::GLOBAL_VARIABLE;
        let mut buf = [0u8; 8];
        for bad in [&[][..], &[0x41], &[0x41, 0, 0x42, 0]] {
            assert_eq!(
                system.variable(bad, &vendor, &mut buf).err(),
                Some(Status::INVALID_PARAMETER),
                "{bad:?}"
            );
            assert_eq!(
                system.set_variable(bad, &vendor, Attributes::default(), &[1]),
                Err(Status::INVALID_PARAMETER)
            );
        }
    }
}
