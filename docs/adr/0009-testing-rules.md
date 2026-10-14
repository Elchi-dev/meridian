# 0009. Test every component with real code, and prove the tests can fail

- Status: accepted
- Date: 2026-10-14

## Context

An operating system that runs other people's files and holds their data has to be tested more thoroughly than a typical application. Two failure modes are common: too few tests, and tests that look like tests but cannot fail, either because they assert nothing that matters or because they run against fake objects instead of the real thing.

## Decision

1. **Ratio.** Every component (each Rust crate and Go module) has at least 1.5 lines of test code for every line of real code. Lines are counted without comments and blank lines. `#[cfg(test)]` code, Rust `tests/` folders, QEMU test code and Go `_test.go` files count as tests. `sextant stats --check` enforces it in CI.
2. **Mutation testing.** `sextant mutate` changes the code on purpose and runs the tests. A change no test notices fails CI, unless it is listed in `mutants.meri` with the reason it cannot change behaviour.
3. **Real implementations only.** Allowed are real implementations behind an interface (a RAM block device that is also the ramdisk, an in-memory file system from the standard library), the emulated hardware in QEMU, and real files that Meridian's own tools build. Not allowed are objects that return canned answers, fake hardware registers, and anything that exists only for a test and pretends to be something else. Hardware drivers are tested in QEMU.
4. **No stubs.** A function that exists only to satisfy an interface and does not do what its name says is not allowed. Code that handles its input and returns a documented error for a case it does not support is not a stub.
5. **Windows behaviour** is tested against real Windows (0004): small test programs run on GitHub's Windows runners, and their output is what Meridian has to reproduce.

## Alternatives considered

- **A coverage threshold.** Coverage shows what ran, not what was checked. It is measured and reported, but the mutation score is the gate.
- **Mocks for hardware.** They make drivers testable without an emulator, but they test the driver against the author's own understanding of the hardware, which is exactly what can be wrong.

## Consequences

Writing a feature takes noticeably longer than writing its code. In exchange, a green CI run means something, and a test that cannot fail is found by a program instead of by a user.
