# 0030. Set when a file last changed through a call, to what FAT keeps

- Status: accepted
- Date: 2026-11-09

## Context

Files keep when they last changed (decision record 0029), but only
Meridian sets it, to the time of day as it writes. `touch` sets it to
now or to a time it is given, `pax` and `cp -p` put back the times of
the files they copy, and a `make` would lean on both. POSIX has
`futimens` and `utimensat` for this, and Windows has `SetFileTime` on a
handle that may write the file's attributes.

FAT keeps a time to two seconds, from 1980 to 2107. Linux's FAT driver
takes a time outside those years as the nearest one it keeps.

## Decision

A call, `SET_CHANGED`, sets when the file of a handle last changed to
the seconds since 1970 began that it is given. The handle must allow
writing, as it must for WRITE_FILE, and a directory opened to write may
be set too. The time is kept as FAT keeps it: cut to two seconds, and a
time before 1980 or past 2107 becomes the first or the last FAT can say,
as on Linux. A handle to what keeps no time, as a volume's root and the
directories of the tree above the volumes, is refused with DENIED.
Every handle open on the file sees the new time, and writing into the
file later stamps it with the time of day again.

## Alternatives considered

- **A path rather than a handle**, as `utimensat` takes. A handle has
  passed the access check for writing already, and the call then works
  for a file a program holds open, as `futimens` does.
- **A time with WRITE_FILE.** Nearly every write wants the time of day,
  and WRITE_FILE keeps the arguments it has.
- **Refusing a time FAT cannot keep.** A program that copies files from
  another system would fail on one from before 1980, which Linux keeps
  as 1980.

## Consequences

`touch`, `pax` and `cp -p` can set when files changed, and so can any
program that may write the file. A time set before 1980 reads back as
1 January 1980, and an odd second as the one before. Strata will keep
finer times later and answer the same call.
