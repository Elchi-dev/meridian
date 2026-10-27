//! The disks the drivers found, each with its partition table and what
//! may be written on it (decision record 0015). Every write to a disk goes
//! through [`write`]: the drivers' own write functions take a [`Permit`],
//! which only this module makes.

use alloc::vec::Vec;
use core::fmt;

use gpt::Partition;
use kernel::disk::{Lists, Table, Writable};

use super::memory::Spin;
use super::{ahci, nvme, virtio};

/// The driver a disk is behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Nvme,
    Virtio,
    /// On the AHCI port it names.
    Ahci(usize),
}

impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Driver::Nvme => f.write_str("NVMe"),
            Driver::Virtio => f.write_str("virtio"),
            Driver::Ahci(port) => write!(f, "AHCI port {port}"),
        }
    }
}

/// What the drivers' write functions take, so that nothing reaches them
/// but [`write`], after its checks.
#[derive(Clone, Copy)]
pub struct Permit(());

/// The permit the checks of the drivers themselves write with, past the
/// rules, to see what each driver refuses on its own.
#[cfg(feature = "qemu-test")]
pub const CHECKS: Permit = Permit(());

struct Disk {
    driver: Driver,
    read_only: bool,
    table: Option<Table>,
    writable: Writable,
}

static DISKS: Spin<Vec<Disk>> = Spin::new(Vec::new());

/// Why a write did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No disk has the number.
    NoDisk,
    /// Meridian may not write there.
    Refused,
    /// The driver could not.
    Failed,
}

/// Reads the sectors from `lba` on into `into`, a whole number of them,
/// from the disk behind `driver`, and says whether it could.
pub fn read_from(driver: Driver, lba: u64, into: &mut [u8]) -> bool {
    match driver {
        Driver::Nvme => nvme::read(lba, into).is_ok(),
        Driver::Virtio => virtio::read(lba, into).is_ok(),
        Driver::Ahci(port) => ahci::read(port, lba, into).is_ok(),
    }
}

/// Takes in the disk behind `driver`, which says whether it only reads,
/// with its partition table, nothing on it writable until [`trust`].
pub fn add(driver: Driver, read_only: bool) {
    let table = Table::read(&mut |lba, into| read_from(driver, lba, into));
    DISKS.lock().push(Disk {
        driver,
        read_only,
        table,
        writable: Writable::default(),
    });
}

/// The disk the install the loader came from is on, as [`home`] finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Home {
    pub disk: usize,
    pub driver: Driver,
    /// Whether the disk says it only reads.
    pub read_only: bool,
    pub system: Partition,
    /// Whether Meridian may write the whole system partition.
    pub writable: bool,
}

/// The disk the install the loader came from is on, with its Meridian
/// system partition: of the disks that hold the partition `boot` names, a
/// virtio one first, as only those take files for now (QUESTIONS.md 9.6),
/// which matters where one disk is reached through several controllers,
/// as in a virtual machine.
pub fn home(boot: [u8; 16]) -> Option<Home> {
    let disks = DISKS.lock();
    let mut found = disks.iter().enumerate().filter_map(|(disk, d)| {
        let system = d.table.as_ref()?.system(boot)?;
        Some(Home {
            disk,
            driver: d.driver,
            read_only: d.read_only,
            system,
            writable: d.writable.allows(system.first, system.sectors()),
        })
    });
    let first = found.next()?;
    Some(
        found
            .chain(core::iter::once(first))
            .find(|h| h.driver == Driver::Virtio)
            .unwrap_or(first),
    )
}

/// The number of the disk behind `driver`.
#[cfg_attr(not(feature = "qemu-test"), allow(dead_code))]
pub fn find(driver: Driver) -> Option<usize> {
    DISKS.lock().iter().position(|d| d.driver == driver)
}

/// Lets each disk be written as the lists on the system partition of the
/// install the loader came from, `boot`, say, and returns the driver of
/// the disk they are on, its [`home`]. Nothing may be written when there
/// is none.
pub fn trust(boot: [u8; 16]) -> Option<Driver> {
    let found = home(boot);
    let lists = found.map_or_else(Lists::default, |h| {
        Lists::read(|lba, into| read_from(h.driver, h.system.first + lba, into))
    });
    for disk in DISKS.lock().iter_mut() {
        disk.writable = Writable::of(disk.table.as_ref(), &lists.made, &lists.unlocked);
    }
    found.map(|h| h.driver)
}

/// Each disk's number, driver, and what may be written on it.
pub fn each(mut f: impl FnMut(usize, Driver, &Writable)) {
    for (number, disk) in DISKS.lock().iter().enumerate() {
        f(number, disk.driver, &disk.writable);
    }
}

/// Writes `from`, a whole number of sectors, to disk `disk` from `lba` on,
/// when Meridian may write all of them there.
///
/// # Errors
///
/// When no disk has the number, Meridian may not write there, or the
/// driver could not.
pub fn write(disk: usize, lba: u64, from: &[u8]) -> Result<(), Error> {
    let driver = {
        let disks = DISKS.lock();
        let disk = disks.get(disk).ok_or(Error::NoDisk)?;
        let sectors = from.len().div_ceil(512) as u64;
        if !disk.writable.allows(lba, sectors) {
            return Err(Error::Refused);
        }
        disk.driver
    };
    let permit = Permit(());
    let done = match driver {
        Driver::Nvme => nvme::write(permit, lba, from).is_ok(),
        Driver::Virtio => virtio::write(permit, lba, from).is_ok(),
        Driver::Ahci(port) => ahci::write(permit, port, lba, from).is_ok(),
    };
    if done { Ok(()) } else { Err(Error::Failed) }
}
