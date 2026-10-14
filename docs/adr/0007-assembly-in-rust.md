# 0007. Write assembly inside Rust, in Intel syntax

- Status: accepted
- Date: 2026-10-14

## Context

Some parts of an operating system can only be written in assembly: the entry point of each CPU, interrupt and system call entry, the 16-bit startup code of the other CPU cores, context switches. They could live in separate `.asm` files assembled by NASM, or inside Rust through `asm!`, `global_asm!` and naked functions, assembled by the LLVM assembler that ships with rustc.

Meridian is built on Linux, macOS and Windows hosts (QUESTIONS.md 4.3), so every extra tool has to be installed and kept at a compatible version on each of them.

## Decision

All assembly is written inside Rust: `asm!` for single instructions inside functions, naked functions for entry points, and `global_asm!` with `include_str!` for longer stretches such as the AP startup code. Intel syntax everywhere, which is rustc's default for x86.

## Alternatives considered

- **NASM files.** NASM's syntax is pleasant, and Samuel's nasmnet shows the style. But it adds a tool to install on every host and in CI, and a link step that Cargo does not know about.
- **AT&T syntax.** The GNU default. Intel syntax matches the Intel and AMD manuals, which is where every instruction is checked.

## Consequences

`cargo build` is the whole build. Assembly sits next to the Rust code that calls it, and the compiler checks the operand types. Each block of assembly is `unsafe` and therefore lives in a `raw` module (0008).
