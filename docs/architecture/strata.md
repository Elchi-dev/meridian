# Strata

Meridian's own file system. Nothing exists yet; the first version is
planned for M4. The on-disk format is designed once, with room for every
feature below, so it never has to break.

## In the first version

| Feature | What it gives a person |
| --- | --- |
| Copy-on-write | a crash never leaves a half-written block in place |
| Checksums on data and metadata | damage is found, not served |
| Snapshots | the time slider in Files, and undo for installers and system changes |
| Names that ignore case but keep it | Windows programs work as they expect; `Report.pdf` stays `Report.pdf` |
| Extended attributes and alternate data streams | what Windows and native apps store next to a file |
| Instant locate index | a B-tree and trigram index over every name, updated on each write and rename, so searching any substring or glob takes under a millisecond with no indexing daemon |

## Reserved in the format, built later

| Feature | What it gives |
| --- | --- |
| Multi-file transactions | apps and installers group many creates, writes and renames into one copy-on-write transaction (`fs_tx_begin`, `fs_tx_commit`, `fs_tx_abort`); a crash never leaves a half-written folder |
| Content-addressed deduplication | identical blocks are stored once, and copying a file is a metadata clone that costs nothing |
| Files that expire | an inode can carry an expiry time, so caches and temporary downloads free their space without a cleanup daemon |
| Encryption | needs Meridian's own cryptography |

Compression is not planned for now.

## Other file systems

1. FAT32, read and write: the EFI partition (M4).
2. Strata (M4).
3. exFAT, read and write: USB sticks.
4. NTFS, read only: files and programs on a Windows partition, and an import for people moving from Windows.
5. ext4, read only.

## Safety on real hardware

Meridian never writes to a partition it did not create unless the person
unlocks that partition in Settings. Until Strata is stable, the AHCI and
NVMe drivers mount everything else read only. On the machines Meridian is
tested on, the Windows and Linux installs share disks with it.
