# 0003. Take no outside code, and justify every outside file

- Status: accepted
- Date: 2026-10-14

## Context

Meridian is written from scratch: bootloader, kernel, file system, desktop, the Windows personality, and the build tool. Outside code would make some parts faster to write, but every dependency is code nobody here has read, with its own release cycle, its own bugs and its own licence. For an operating system, that is the code that runs with the most privilege on the machine.

Some things cannot be written by us at all. GPUs and Wi-Fi chips refuse to work without the vendor's signed firmware. Secure Boot only trusts binaries signed by Microsoft's UEFI certificate authority. A typeface takes years to design.

## Decision

1. **No outside code.** No crates, no Go modules, no vendored libraries, no code copied from other projects, in anything that ships and in the tools used to build it. This includes the hard parts: cryptography and TLS are written here, tested against the official vectors of their specifications.
2. **What comes with the toolchains is not a dependency.** Rust's `core` and `alloc`, Go's standard library, and the compilers themselves are what everything is written in.
3. **Outside files are allowed only with a record.** Firmware blobs, fonts, the signed Secure Boot shim, keyboard layout data and similar data each get a decision record before they land: where the file comes from, its licence, its checksum, and why writing it ourselves is impossible or pointless. They live under `third-party/`.
4. **Driver code from elsewhere** may be studied or translated into Rust only from sources under MIT, BSD or ISC style licences, with attribution and a record (see 0002 for why GPL is out). No C enters the build.
5. **CI actions** are outside code too and follow rule 3.

`sextant lint` enforces this: it fails if `Cargo.lock` lists any package from a registry, if `go.mod` requires a module, if a workflow uses an action without a record, or if a file under `third-party/` has no record pointing to it.

## Alternatives considered

- **Few, carefully chosen dependencies.** The usual advice, and right for most projects. Here it would undercut the point of the project and make "written from scratch" untrue.
- **Dependencies for tooling only** (`cargo-mutants`, `cargo-llvm-cov`). They would save weeks, but the mutation tester and line counter are small, well specified programs, and building them into `sextant` keeps every machine on the same exact behaviour.

## Consequences

Every subsystem takes longer. In exchange there is no supply chain to attack, no licence surprises, and every line can be read and explained by whoever works on Meridian.
