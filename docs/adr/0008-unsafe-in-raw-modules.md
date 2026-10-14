# 0008. Keep all unsafe code in raw modules

- Status: accepted
- Date: 2026-10-14

## Context

A kernel cannot avoid `unsafe`: port I/O, model-specific registers, page tables, firmware calls and every piece of assembly need it. Rust's guarantees only hold if each `unsafe` block upholds its invariants, and that is easiest to check when unsafe code is small, in one place, and separate from the logic around it.

Samuel's rule for this project (QUESTIONS.md C5): separate unsafe and safe code, never mix them in one file.

## Decision

Each crate keeps all its unsafe code in a module named `raw`, either `src/raw.rs` or a folder `src/raw/`. Raw modules expose safe functions whose callers cannot break the invariants, and every `unsafe` block inside them carries a `// SAFETY:` comment that says why it holds. Crates deny `unsafe_code` and allow it only on their `raw` module.

`sextant lint` fails on any `unsafe` outside a `raw` module, clippy's `undocumented_unsafe_blocks` fails on a block without its comment, and `sextant stats` reports the number of `unsafe` uses per crate, so the count is visible in every review.

## Alternatives considered

- **Unsafe wherever it is needed, documented.** The usual practice. It scatters the code that needs the closest review across the whole crate.
- **Separate crates for unsafe code.** Stronger separation, but many tiny crates and awkward visibility for code that is only used once.

## Consequences

Reviewing the unsafe surface of a crate means reading its `raw` module. Safe code never needs to think about it. Some wrappers will feel thin, which is the point.
