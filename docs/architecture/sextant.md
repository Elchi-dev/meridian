# sextant

The build and development tool, in `tools/sextant/`: one Go program, only
the standard library, no shell scripts and no Unix-only tools. It runs on
Linux (x86_64 and arm64), macOS and Windows, because Meridian is built on
more than one machine and none of them may be special.

sextant is the outside tool. A tool that runs inside Meridian to control the
system will get its own name later.

## Commands

| Command | What | State |
| --- | --- | --- |
| `help`, `version` | the overview, and the version with the git revision it was built from | works |
| `stats [--check] [--files] [--min N]` | code and test lines per component; `--check` fails below the minimum and says how many test lines are missing | works |
| `lint` | the project rules no compiler knows | works |
| `doctor` | checks every tool and version, prints how to fix each, and installs what is missing after one "y" | being built |
| `build [part] [--release]` | builds the loader, and later the kernel and everything else, through Cargo | being built |
| `image` | writes a bootable GPT disk image, and an ISO | being built; the writers work |
| `run` | boots the image in QEMU with KVM, HVF or WHPX when available, serial output in the terminal | being built |
| `test [--host] [--qemu] [--win]` | host tests, QEMU test images, Windows behaviour tests | being built |
| `mutate [paths]` | mutation testing for Rust and Go | being built |
| `debug` | like `run`, paused, with GDB attached | planned |
| `adr new "title"` | the next numbered decision record from the template | planned |
| `flash DEVICE` | writes the image to a USB stick; refuses any disk that is not removable or has mounted partitions, and asks for the device's size to be typed back | planned |
| `ci [--list]` | every check CI runs, in its order, stopping at the first that fails; CI's linux job runs it, so the two cannot drift apart | works |
| `release` | release artefacts with checksums | planned |

## Settings

`sextant.meri` at the root of the repository marks the root and holds the
shared settings. `sextant.local.meri` next to it, ignored by git, holds the
settings of one machine, such as the build folder or the path to OVMF. It
is layered over the shared file by the rules in
[the format](../formats/meri.md).

## Counting lines

- **Components.** Every Rust crate (a `Cargo.toml` with a `[package]` table) and every Go module.
- **What counts.** Lines with code count. Comment lines and blank lines never count.
- **What counts as a test:**
  - Rust code under `#[cfg(test)]`, or under a `cfg` that names the `qemu-test` feature
  - files marked `#![cfg(test)]`
  - the files of test modules
  - the crate's `tests/` and `benches/` folders
  - Go `_test.go` files
- **Skipped folders.** `target`, `out`, `testdata`, `node_modules` and hidden folders.
- **`unsafe`.** It is counted per crate, so the number is visible in every review.

## The lint rules

- **Dashes.** No em dash, en dash, figure dash or horizontal bar in any text file.
- **Unfinished work.** No `TODO`, `FIXME`, `XXX` or `HACK` in comments; open work goes into `TODO.md`. No `todo!()` or `unimplemented!()`.
- **`unsafe`.** Only in a `raw.rs` file or a `raw/` folder.
- **Outside code.** No crate from a registry or git in `Cargo.lock`, no required module in `go.mod`, no CI action, and no file under `third-party/`, unless an accepted decision record allows it on an `- Allows:` line.

## Disk images

GPT with the protective MBR, FAT32 for the EFI partition, and ISO 9660 with
an El Torito entry for UEFI. All three are written by sextant itself, and
the same inputs give the same bytes on every machine (ADR 0010).

## Signing

Every image carries a loader signed for Secure Boot. sextant computes the
loader's Authenticode hash, signs it with RSA-2048 and SHA-256 in a PKCS #7
SignedData, and puts that in the certificate table, all with Go's standard
library (`internal/authenticode`). The key and its certificate come from the
PEM file `sign.key` names. Without one, sextant makes a development key the
first time it signs and keeps it in the user's settings folder
(`~/.config/sextant/development-key.pem` on Linux), readable only by its
owner. A signature does not change from one run to the next, so with the
same key the same commit still gives the same image; with another key,
only the signature differs.

The tests check the hash against a second reading of the file through Go's
`debug/pe`, and have OpenSSL verify the signature, the signed digest and
the certificate.

## QEMU

`run` and `test` start QEMU with:

- the q35 machine and the OVMF firmware, with a fresh copy of its variable store for every run
- the image as a disk
- the serial port on the terminal or in a log
- for test images, the `isa-debug-exit` device on port 0xf4

The accelerator is chosen per host: KVM on Linux, HVF on Intel Macs, WHPX
on Windows, and TCG, which is slow, where none exists. On an ARM machine
every x86_64 instruction is emulated, which is 10 to 50 times slower.
