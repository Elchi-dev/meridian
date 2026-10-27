# 0015. Write only to partitions Meridian made, and to others once unlocked

- Status: accepted
- Date: 2026-10-27

## Context

QUESTIONS.md 9.6 settled the rule: Meridian never writes to a partition
it did not create unless the person unlocks that partition in Settings,
and until Strata is stable the AHCI and NVMe drivers mount everything
read only. The PC Meridian is developed on boots Windows and Arch Linux
from the same disks, so a wrong write there loses real data.

Today the kernel can write through all three drivers, a command of up to
sixteen frames at a time, but only the test image does, and only to the
test disk's Meridian system partition and to copies QEMU throws away.
The tree of files mounts every volume read only. `lib/fat` can already
write FAT32, so the first writes through the tree of files are close,
and the rule needs a mechanism before they land:

- The kernel has to tell a partition Meridian made from one it did not.
  A GPT entry carries a type GUID, a unique GUID of 128 random bits, its
  first and last sector, 64 attribute bits and a name.
- An unlock has to be stored somewhere the person can see and undo it.
- The check has to sit where nothing can go around it. The drivers'
  write functions take any sector of the disk.

## Decision

A partition counts as Meridian's when its unique GUID is listed in
`made.meri` on the Meridian system partition of the running install: the
one on the disk the loader started from. A list on any other disk does
not count. Meridian adds a partition there when it creates one, the
installer and sextant included (the system partition itself and the EFI
system partition it made come first), and never for one it finds. An unlock lists the partition's unique GUID in
`unlocked.meri` next to it, which Settings writes, with the partition's
name and size as the person saw them when they agreed.

Every write goes through the kernel's disk layer, which knows each
partition's sectors and whether it may be written: one of Meridian's, or
an unlocked one. The drivers' write functions are no longer reachable
from anywhere else, and a write that reaches past the end of its
partition is refused like one to a partition that may not be written.
The GPT itself is written only on a disk whose every partition is
Meridian's. A disk without a GPT has no unique GUIDs, so nothing on it
is Meridian's.

Above the disk layer, the tree of files mounts every volume on an AHCI
or NVMe disk read only until Strata is stable, so until then files are
written only on virtio disks. The disk layer itself still lets the
kernel's own checks write to Meridian's partitions behind all three
drivers.

## Alternatives considered

- **The type GUID alone**, `gpt::MERIDIAN_SYSTEM` and the types that
  follow it. Simple and needs no file, but the EFI system partition a
  Meridian install creates has the same type as the one Windows created,
  and anyone can give a partition Meridian's type.
- **GPT attribute bits.** Bits 48 to 63 belong to the partition's type,
  so the meaning of a bit Meridian picks is not Meridian's to give on an
  EFI system partition, and other tools may clear or set them.
- **A marker file inside the partition.** It needs a write to make, so
  it cannot come before the rule it serves, and a file system another
  system owns may hold one by accident or on purpose.

## Consequences

The unlock is visible and can be undone by removing a line. A Meridian
started from a USB stick reads the stick's lists, so it cannot vouch for
a partition on the PC's disks that it did not make itself. Wiping or
recreating a partition gives it a new unique GUID, so an unlock does not
carry over to whatever takes its place. A Meridian system partition is
needed before anything else can be written, which the installer creates
first. The disk layer becomes the only way to a driver's writes, and its
checks need their own QEMU checks: a write refused to a partition that is
not Meridian's, to one past the end of an allowed one, and to the GPT of
a disk that is not all Meridian's.
