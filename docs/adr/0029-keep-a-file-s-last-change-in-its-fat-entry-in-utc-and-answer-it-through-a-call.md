# 0029. Keep a file's last change in its FAT entry, in UTC, and answer it through a call

- Status: accepted
- Date: 2026-11-09

## Context

Meridian has the time of day now (decision record 0028), and programs
want to know when a file last changed: pax writes it into archives,
`test -nt` and `find -newer` compare it, `touch` sets it, and a `make`
would build on nothing else.

Meridian's volumes are FAT32 for now. A FAT entry keeps when it was
made, to a hundredth of a second, the day it was last read, and when it
last changed, to two seconds, from 1980 to 2107. The specification
keeps them in local time without saying which zone; Windows writes
local time, and Linux writes UTC unless it is told a zone. Meridian's
FAT writer has stamped every entry with 1 January 1980 so far.

## Decision

The FAT writer is given the time of day when it writes, and stamps an
entry it makes with it as made, read and changed, and a file it writes
data into as changed, in UTC, as Linux does. With no time of day known
it stamps 1 January 1980, as it did. Directories keep the time they
were made.

A call, `CHANGED`, answers when the file of a handle last changed, as
seconds since 1970 began, with rdx 1; or 0 with rdx 0 for what keeps no
time, such as the directories of the tree above the volumes.

## Alternatives considered

- **Local time in the entries.** What Windows shows would match its own
  zone, but Meridian knows no zones yet, and a volume carried between
  machines would change its times with them.
- **The time in INFO's answer.** INFO answers the size in rax and the
  kind in rdx, and neither has room for a time.
- **A table of times kept beside the volume.** It would be lost the
  moment another system writes the volume, and FAT has the fields.

## Consequences

pax, `test` and `touch` can use real times, and Strata will answer the
same call later with its own finer times. Windows shows the times
Meridian writes off by its zone, as it shows Linux's. A time is only as
right as the time of day was when the file changed, and FAT's two
seconds are the finest it gets.
