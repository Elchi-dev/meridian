//! The PC's real-time clock at ports 0x70 and 0x71 (decision record
//! 0033): the time it holds, and a time set written into it.

use acpi::RealTimeClock;
use rtc::{
    DAY, HOURS, MINUTES, MONTH, Registers, SECONDS, SET, STATUS_A, STATUS_B, UPDATING, YEAR,
};

use super::memory::Spin;

/// The port that picks a register of the CMOS RAM, and the one that reads
/// and writes it.
const INDEX: u16 = 0x70;
const DATA: u16 = 0x71;

/// The clock, once the FADT said whether there is one; held while its
/// registers are read or written, which keeps a register picked and then
/// read or written together.
static CLOCK: Spin<Option<RealTimeClock>> = Spin::new(None);

/// The most looks at register A while the clock updates its registers,
/// which takes it under two milliseconds; past them it is read anyway.
const PATIENCE: u32 = 1_000_000;

/// Keeps what the FADT said of the clock, `rtc`, for [`read`] and
/// [`write`].
pub fn keep(rtc: Option<RealTimeClock>) {
    *CLOCK.lock() = rtc;
}

fn register(index: u8) -> u8 {
    // SAFETY: the CMOS RAM's ports on every PC; picking a register and
    // reading it changes no time.
    unsafe {
        x86::raw::write_port_u8(INDEX, index);
        x86::raw::read_port_u8(DATA)
    }
}

fn set_register(index: u8, value: u8) {
    // SAFETY: the CMOS RAM's ports, and a register of the time, which the
    // caller holds still with register B's SET bit.
    unsafe {
        x86::raw::write_port_u8(INDEX, index);
        x86::raw::write_port_u8(DATA, value);
    }
}

/// Waits until no update of the registers is in progress.
fn settled() {
    for _ in 0..PATIENCE {
        if register(STATUS_A) & UPDATING == 0 {
            return;
        }
    }
}

/// The time's registers, and the century's when `century` names it.
#[cfg(feature = "qemu-test")]
fn registers(century: Option<u8>) -> Registers {
    Registers {
        seconds: register(SECONDS),
        minutes: register(MINUTES),
        hours: register(HOURS),
        day: register(DAY),
        month: register(MONTH),
        year: register(YEAR),
        century: century.map_or(0, register),
    }
}

/// The time the clock holds, as seconds since 1970 began; none without a
/// clock, or when its registers make no time.
#[cfg(feature = "qemu-test")]
pub fn read() -> Option<u64> {
    let clock = CLOCK.lock();
    let century = (*clock)?.century;
    // Read twice and again while an update came between, which would mix
    // two times.
    let mut read = registers(century);
    for _ in 0..3 {
        settled();
        let again = registers(century);
        if again == read {
            break;
        }
        read = again;
    }
    read.seconds(register(STATUS_B), century.is_some())
}

/// Writes `seconds` since 1970 began into the clock, in the form register
/// B says it keeps; false without a clock, or for a year past 9999.
pub fn write(seconds: u64) -> bool {
    let clock = CLOCK.lock();
    let Some(RealTimeClock { century }) = *clock else {
        return false;
    };
    settled();
    let status = register(STATUS_B) & !SET;
    let Some(set) = Registers::of(seconds, status) else {
        return false;
    };
    set_register(STATUS_B, status | SET);
    for (index, value) in [
        (SECONDS, set.seconds),
        (MINUTES, set.minutes),
        (HOURS, set.hours),
        (DAY, set.day),
        (MONTH, set.month),
        (YEAR, set.year),
    ] {
        set_register(index, value);
    }
    if let Some(index) = century {
        set_register(index, set.century);
    }
    set_register(STATUS_B, status);
    true
}
