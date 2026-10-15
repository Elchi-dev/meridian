# The .mexec format

Version 1

`.mexec` is how Meridian's own programs are stored: an ELF64 file for
x86_64, position independent, that carries Meridian's notes. Everything
that reads ELF keeps working on it, `readelf`, `objdump` and `gdb` among
them; the notes are what the kernel looks for before it starts a file,
and later where the icon, the permissions and the sandbox go.

```text
$ readelf -n hello.mexec

Displaying notes found in: .note.meridian
  Owner                Data size        Description
  Meridian             0x00000004       NT_VERSION (version)   description data: 01 00 00 00
  Meridian             0x00000005       NT_ARCH (architecture) description data: 68 65 6c 6c 6f
```

`readelf` names the kinds after the ones the GNU tools use for 1 and 2;
for the owner `Meridian` they mean what this page says.

## The file

- An ELF64 file, little endian, for x86_64 (machine 62), of type `ET_DYN`:
  the kernel places it where it wants and applies its `R_X86_64_RELATIVE`
  relocations. A file of type `ET_EXEC` has to be linked to start at 4 MiB.
- Its loadable segments, in ascending order and not overlapping. A page
  is mapped with what every segment on it allows, and a page that would be
  writable and executable at once is refused, so the linker has to put
  code and data on pages of their own, as it does by default.
- The whole image fits between 4 MiB, where it starts, and the stack below
  8 MiB: 4,124,672 bytes at most.
- Its entry point lies in an executable segment.

## The notes

The notes lie in a `PT_NOTE` segment, aligned to 4 bytes, in a section
called `.note.meridian` by convention. Each is an ELF note: the length of
the owner's name with its zero byte, the length of the description, the
kind, then the name and the description, each padded to 4 bytes. The
owner is `Meridian`, nine bytes with the zero. Notes of other owners are
left alone.

| Kind | Name in `lib/abi` | Description |
| --- | --- | --- |
| 1 | `NOTE_VERSION` | the format's version, a 32-bit little-endian number: 1 |
| 2 | `NOTE_NAME` | the program's name in UTF-8, without a zero byte |

A file is a `.mexec` file of version 1 when it carries a note of kind 1
whose description is the four bytes `01 00 00 00`. The kernel refuses an
ELF file without that note, and one whose note names a version it does not
know. A name is optional; the kernel keeps up to 32 bytes of it for its
log. Kinds it does not know are skipped, so later versions can add notes
that older kernels pass over.

## Writing one

A program built on `lib/program` gets its notes from `main!`, which
writes them with the crate's name as the program's name:

```rust
#![no_std]
#![no_main]

use program::Handle;

program::main!(hello);

fn hello(_: Handle) -> u64 {
    let _ = program::log("Hello from ring 3, in Rust");
    0
}
```

sextant builds every program for `x86_64-unknown-none` and puts it into
`meridian/programs` on the EFI partition under its name with `.mexec` at
the end.

## Still to come

The icon, the permissions a program asks for, and whether it runs in a
sandbox (QUESTIONS.md 7.6 and 7.10) each become a kind of their own.
