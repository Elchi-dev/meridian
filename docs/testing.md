# Testing

The rules are in [ADR 0009](adr/0009-testing-rules.md). In short:

- at least 1.5 lines of test code per line of code in every component
- tests that are proven able to fail
- no mocks, no stubs

This page is how the tests are actually run.

## Host tests

Everything that is plain logic runs on the build machine in milliseconds:

```
cargo test --workspace                      # Rust: lib/uefi, boot
cd tools/sextant && go test -race ./...     # Go: sextant
```

Rust crates are written `#![cfg_attr(not(test), no_std)]`, so the same code
that runs in the loader or the kernel is tested with the standard test
harness on the host.

### Tests that use real tools as readers

sextant's disk image writers, and Meridian's QR codes, are checked against
independent readers:

- GPT: `sfdisk --json` and `--verify`, and `fdisk -l`
- FAT32: `fsck.fat -n`, `fatlabel`, `mdir` and `mcopy`
- ISO: `xorriso -report_el_torito` and `-extract`, and `bsdtar`
- QR codes: `zbarimg` must read back what `lib/qr` encoded, at every
  version and level, and every module must match what `qrencode` makes
  from the same bytes once both use the same mask

Where a tool is missing, its check is skipped with a message naming the
tool. With `SEXTANT_REQUIRE_TOOLS=1`, which CI on Linux sets, a missing tool
fails the test instead.

## QEMU tests

Code that talks to firmware or hardware is tested against real firmware and
emulated hardware in QEMU, never against simulated registers. A test image
is a normal build with the `qemu-test` feature on. It runs its checks and
ends QEMU through the `isa-debug-exit` device.

### The serial protocol

Every line goes to COM1, and sextant reads them from the serial log:

```
TEST uefi.vendor ok
TEST uefi.console.write ok
TEST boot.stall FAILED Stall(1000) failed
TESTS DONE 1 failed
```

- `TEST <name> ok` for a passed check.
- `TEST <name> FAILED <why>` for a failed one, saying what a failure means.
- `PRESS <key>` asks sextant to type a key, named as the QEMU monitor
  names it, such as `ret` or `down`.
- `SCREEN <name>` asks sextant to keep a picture of the screen. sextant
  saves it as `screens/<name>.png` in the build directory and then sends a
  carriage return on the serial port, which the image waits for before it
  draws anything else. Under the firmware it arrives as the Enter key. Names are lower case letters, digits and dashes.
- `TESTS DONE <n> failed` once at the end.
- **The exit code.** The image then writes to port 0xf4. QEMU exits with
  `(value << 1) | 1`: **33** when everything passed, **35** when something
  failed. Any other exit code, or no `TESTS DONE` line before the timeout,
  means the image crashed or hung, which also counts as a failure.

### Checking what reached the screen

The kernel's test image checks its own screens by reading the framebuffer
back, and keeps a picture of each with `SCREEN`. A Go test of the runner
opens the saved status screen and checks the colours at known places, so
what a person would see is tested too, not only what the kernel believes
it drew.

The image people boot has no checks and asks for nothing, so a Go test's
script keeps its screens instead: a script step can wait for a line, pause
and keep the screen under a name. The test of the plain image keeps the
status screen twice, 2.5 seconds apart, and checks that only the uptime
changed in between.

OVMF copies its text console to the serial port. So text a test writes to
the firmware console also appears in the serial log, exactly as the console
received it. The `uefi.console.write` check uses this: it writes a probe
longer than one 128-unit piece of the console encoder, and the probe only
arrives whole if splitting it into pieces works. sextant compares the log
with the expected text.

### Where QEMU tests live

Each crate owns its QEMU checks, in a module behind its own `qemu-test`
feature (`lib/uefi/src/qemu_test.rs`). The loader's test image runs all of
them and adds its own (`boot/src/qemu_test.rs`). Because the module sits
under a `qemu-test` cfg, its lines count as test code for that crate.

### Running one by hand

`sextant test --qemu` does all of this. To boot a test image by hand,
run this from the repository root, with OVMF from Arch Linux's
`edk2-ovmf` package (other systems keep it elsewhere):

```
cargo build -p boot --target x86_64-unknown-uefi --features qemu-test
cargo build -p kernel --target x86_64-unknown-none --features qemu-test
mkdir -p out/esp/EFI/BOOT out/esp/meridian
cp target/x86_64-unknown-uefi/debug/meridian.efi out/esp/EFI/BOOT/BOOTX64.EFI
cp target/x86_64-unknown-none/debug/kernel out/esp/meridian/kernel
cp /usr/share/edk2/x64/OVMF_VARS.4m.fd out/vars.fd
qemu-system-x86_64 -machine q35 -accel kvm -m 256M -nodefaults \
  -display none -no-reboot -serial stdio \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.4m.fd \
  -drive if=pflash,format=raw,file=out/vars.fd \
  -drive format=raw,file=fat:rw:out/esp \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04
echo $?    # 33 means every check passed
```

`fat:rw:` is QEMU's virtual FAT drive made from a folder. It is handy for
this, but `sextant run` boots a real GPT image that sextant writes.

## Windows behaviour (planned, M6)

Each Windows behaviour test is a small program in `tests/win32/`, built with
`clang-cl` and `lld-link`. CI runs it on GitHub's Windows runners and keeps
its output as the reference. Meridian has to produce the same output. This
is how behaviour the documentation does not describe gets decided, without
ever looking at another implementation (ADR 0004).

## Mutation testing

`sextant mutate` proves that tests can fail. It changes the code on purpose
and runs the tests of that component after each change:

- a comparison or boundary flipped (`<` to `<=`)
- `&&` to `||`
- `+` to `-`
- `true` to `false`
- a function body replaced by a default result

**What happens to a mutant:**

- **Killed:** the tests failed, which is what should happen.
- **Unviable:** the mutant does not build. It is reported, not counted.
- **Timed out:** it counts as killed.
- **Survived:** a change that no test notices fails CI.

The only way out for a survivor is an entry in `mutants.meri` at the root of
the repository. Each entry says why the change cannot alter behaviour, for
example a bounds check whose last iteration ends the loop either way. Every
other survivor gets a better test, or simpler code that has no equivalent
mutant.

CI mutates the files changed by each push, and everything once a night.
