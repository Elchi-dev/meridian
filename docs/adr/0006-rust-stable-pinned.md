# 0006. Build with stable Rust, pinned to one exact version

- Status: accepted
- Date: 2026-10-14

## Context

Operating system projects in Rust have traditionally used nightly for `build-std`, the `x86-interrupt` calling convention and custom test frameworks. Nightly changes every day, and a project pinned to an old nightly slowly drifts away from the ecosystem, while one that follows nightly breaks without warning.

Meridian is built on at least two machines and in CI, and every one of them must produce the same result.

What Meridian needs is on stable today:

- `x86_64-unknown-none` and `x86_64-unknown-uefi` ship prebuilt `core` and `alloc`, so no `build-std`
- `asm!`, `global_asm!` and naked functions for the code that has to be assembly
- `core::mem::offset_of!` to check firmware and hardware struct layouts against their specifications

## Decision

`rust-toolchain.toml` pins one exact stable release, `1.99.0`, with `rustfmt`, `clippy`, `llvm-tools` and both bare-metal targets. rustup installs it automatically on any machine that builds Meridian. Moving to a newer release is a normal commit that changes this file and passes CI.

Interrupt entry points are written as naked functions instead of using the unstable `x86-interrupt` calling convention. Tests run on the host with the standard test harness, and in QEMU with Meridian's own runner.

## Alternatives considered

- **Nightly pinned to a date.** Gives `build-std` and custom test frameworks, neither of which Meridian needs. It also brings a toolchain that is harder to reproduce and that tools and CI images rarely have cached.
- **Following the latest stable.** Two machines building a week apart could use different compilers.

## Consequences

Every machine and CI run uses the same compiler. Features that only exist on nightly are not available; when one becomes necessary, that needs its own record.
