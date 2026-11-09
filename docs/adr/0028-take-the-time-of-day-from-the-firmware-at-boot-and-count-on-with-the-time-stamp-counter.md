# 0028. Take the time of day from the firmware at boot and count on with the time stamp counter

- Status: accepted
- Date: 2026-11-09

## Context

Meridian knows how long it has been up, from the time stamp counter
measured against the ACPI timer or the HPET, but not what time it is.
Programs want dates: `date`, the times of files, the times pax writes
into an archive, `ls -l`.

A PC keeps the time of day in its real-time clock, which a battery runs
while the machine is off. UEFI reads it with the runtime service
`GetTime`: year, month, day, hour, minute, second and nanosecond, the
zone the clock keeps as minutes from UTC, or 2047 when it does not say,
and whether daylight saving is on. Windows keeps the real-time clock in
local time by default, Linux and most firmware setup screens in UTC.

Runtime services may be called after the loader leaves boot services
only with the firmware's own memory mapped as it asks, through
`SetVirtualAddressMap` or by keeping its mappings. Meridian's kernel
keeps neither.

## Decision

The loader calls `GetTime` once, just before it leaves boot services,
and reads the time stamp counter right after. The boot information
carries the seconds since 1970 began in UTC, the nanoseconds past them,
and that reading. The clock's fields are taken as UTC whatever zone it
names, as Linux and edk2's own conversion take them: most firmware
names none, and firmware that names one does not agree on which way it
counts. When the firmware gives no time, or one that is no date, the
boot information says the time is not known.

The kernel answers the time as that moment and the counter's ticks
since, at the rate it measured, through a call of its own, and that it
is not known when it is not. Leap seconds are not counted, as POSIX's
time does not count them. Nothing sets the time yet.

sextant starts test machines with their real-time clock at a fixed
moment, so the checks know the date they should see.

## Alternatives considered

- **Reading the real-time clock in the kernel.** Ports 0x70 and 0x71,
  digits in BCD or binary, 12 or 24 hours, and a flag to wait out while
  the clock updates: the firmware does all that already and knows its
  board, and `GetTime` also answers on machines whose clock is no such
  chip.
- **Keeping the runtime services for the kernel.** It could ask the
  firmware again at any time, but would map the firmware's memory for
  good and run its code in ring 0 in the middle of the kernel. One
  reading and the counter are enough until something sets the time.
- **Time from the network.** Meridian has no network yet.

## Consequences

`date` and the times of files and archives become possible. The time
drifts as far as the counter's measured rate is off, a few seconds a
day at most. A machine whose clock keeps local time, as Windows sets
it, is off by its zone, as Linux shows it. Setting the time, by hand or
from the network later, needs a call that changes the moment the
kernel counts from.
