# 0031. Set the time of day through a call that SeSystemtimePrivilege allows, until the machine stops

- Status: accepted
- Date: 2026-11-09

## Context

The kernel takes the time of day from the firmware's clock at boot and
counts on with the time stamp counter (decision record 0028). A clock
that runs wrong, or a machine whose firmware clock was never set, needs
the time put right, and POSIX's `date` sets it from an operand. Windows
lets `SetSystemTime` do it only for a token with
`SeSystemtimePrivilege`, which Meridian's tokens carry already
(decision record 0014).

Writing the time back into the firmware's clock needs either UEFI's
runtime services, which the kernel does not keep after the loader
leaves, or a driver for the PC's real-time clock, which Meridian does
not have.

## Decision

A call, `SET_TIME`, takes the nanoseconds since 1970 began, in UTC, and
counts the time of day on from them as from the loader's reading. Only
a program whose token holds `SeSystemtimePrivilege` may make it; any
other is answered DENIED, as is a call while the kernel cannot count
time at all, having no rate for the time stamp counter. The time set
lasts until the machine stops: the firmware's clock is left as it is,
and the next boot reads it again.

## Alternatives considered

- **Writing the PC's real-time clock through its ports.** It would keep
  the time across a restart, but it is a driver of its own, with its
  binary and decimal forms, its update cycle and the century the
  firmware may keep elsewhere, and machines without it exist. It can
  come later behind the same call.
- **Calling UEFI's SetTime.** The runtime services would have to stay
  mapped, at the addresses the firmware chose or ones set through
  `SetVirtualAddressMap`, for one call that is rarely made.
- **No privilege, as the time is no file.** Every file's time and every
  check that measures time leans on the clock, so it is as much the
  administrator's as the disk is.

## Consequences

`date` sets the time for a token that may. A time set is gone at the
next boot until Meridian writes the firmware's clock, which a driver
for the real-time clock or the runtime services will do. The first
call that a privilege rather than a handle allows is a model for those
that follow.
