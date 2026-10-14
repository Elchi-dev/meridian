# 0005. Write settings and system files in .meri

- Status: accepted
- Date: 2026-10-14

## Context

Meridian needs one text format for sextant's settings, the bootloader's settings, and later every settings file of the system, the ones the Settings app writes and people also edit by hand. It becomes the system's file type, so it has to be readable by people and quick for programs, and it has to stay stable once systems depend on it.

What the format has to carry is known: sizes and durations appear in almost every system setting (memory, timeouts, refresh rates, scale factors); errors have to point at a line and a column; and the system's own reader will be Rust without the standard library, written from the specification alone. ADR 0003 also means that whatever format is chosen, its parser is written here.

## Decision

Meridian uses `.meri`, specified in `docs/formats/meri.md` with a conformance suite in `tests/meri`:

- `key = value` lines and blocks `name { }` with optional labels, which may repeat
- typed values: booleans, words for fixed choices, integers in four bases, floats, quantities with units (`512MiB`, `30s`, `60Hz`), strings, multi-line text, lists
- one pass without backtracking, a nesting limit of 64 so a reader can work with a fixed stack, and the first error only, with a stable code and a message that says what to do
- layering rules for local overrides

The Go reader lives in `tools/sextant/internal/meri`. A Rust reader for the bootloader and the system follows and must pass the same suite.

## Alternatives considered

- **TOML.** The closest fit and familiar from Cargo, which is a real advantage. But tables, dotted keys, inline tables and arrays of tables give several ways to write the same structure, which a Settings app rewriting files would have to guess around. A conforming parser also needs four date and time types and four string kinds, more than `.meri` needs in total. There are no units, so sizes become bare byte counts or strings.
- **JSON.** Every tool reads it, which nothing else here can claim. No comments, every key in quotes and no trailing commas make it unpleasant to edit by hand, and settings files are edited by hand.
- **YAML.** Readable at first sight, but indentation carries meaning, implicit typing turns `no` into false, and the specification is large enough that implementations disagree with each other.
- **KDL.** The closest in spirit, and well designed. Its documents are nodes with arguments and properties, so a plain setting becomes `memory 512` and a reader has to know which argument means what. The move from version 1 to 2 also shows the format is still changing. No units.
- **HCL.** Its blocks with labels are where `.meri` labels come from. HCL is a configuration language with expressions, interpolation and functions, far more than settings need and much more to implement and secure.

## Consequences

Where `.meri` is worse, today: no editor highlights it, no language server knows it, and everyone who opens a Meridian settings file has to learn one more syntax. There is no schema language, no date or time type, numbers stop at 64 bits, and a file cannot include another.

Two readers have to be kept in step, Go and Rust; the shared suite is what keeps them honest, so every change to the format touches the specification, the suite and both readers. Writing files back while keeping comments, which the Settings app needs, is not part of this decision and comes with the Rust reader.
