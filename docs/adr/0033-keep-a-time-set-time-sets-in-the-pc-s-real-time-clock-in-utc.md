# 0033. Keep a time SET_TIME sets in the PC's real-time clock, in UTC

- Status: accepted
- Date: 2026-11-09

## Context

`SET_TIME` sets the time of day for a token with
`SeSystemtimePrivilege` (decision record 0031), but only until the
machine stops: the next boot reads the firmware's clock again, which
still holds the old time. The firmware's clock on a PC is the real-time
clock of the IBM PC AT, the MC146818 and the chips that copy it, which
answer at I/O ports 0x70 and 0x71 and keep the time in registers of
their CMOS RAM: seconds, minutes, hours, the day, the month and the
year, in binary or in binary-coded decimal and in 12 or 24 hours as
their status register B says, and the century in a register the
ACPI FADT names, if any. ACPI 5 lets the FADT say a machine has no such
clock.

UEFI's `GetTime`, which the loader reads (decision record 0028), reads
the same clock on a PC.

## Decision

A call, `SET_TIME`, takes the nanoseconds since 1970 began, in UTC, and
counts the time of day on from them as from the loader's reading. Only
a program whose token holds `SeSystemtimePrivilege` may make it; any
other is answered DENIED, as is a call while the kernel cannot count
time, having no rate for the time stamp counter. It then writes the
time into the real-time clock, in UTC as Meridian reads it, when the
FADT does not say there is none: it waits out an update in progress, holds
the clock with the SET bit of register B, writes each register in the
form register B gives, the century too where the FADT names its
register, and lets the clock run again. The arithmetic of the registers
lives in `lib/rtc`, apart from the ports.

## Alternatives considered

- **UEFI's `SetTime`.** It needs the runtime services mapped for the
  kernel, for one call; the real-time clock is two ports.
- **Local time in the clock**, as Windows keeps it. Meridian knows no
  zones yet, and reads the clock as UTC already.
- **Writing the clock only when asked**, as `hwclock --systohc` does.
  A time set that the next boot forgets surprises more than one that
  stays.

## Consequences

A time set with `date` stays across a restart on a PC. Windows on the
same machine shows the clock off by its zone, as it does after Linux.
A machine whose FADT says it has no real-time clock keeps a time set
until it stops, as before.
