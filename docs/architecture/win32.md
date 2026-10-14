# The Windows personality

How Meridian runs Windows programs. Nothing on this page exists yet; it is
planned for M6 (console programs) and M8 (windows).

## Approach

Meridian does not translate Windows into another system. The kernel's
native model already is handles to objects with access rights, so the
Windows layer is a set of DLLs written in Rust, built as real PE DLLs:

```
  program.exe
     |  imports
     v
  kernel32.dll  user32.dll  gdi32.dll  ucrtbase.dll ...   Meridian's own, in Rust
     |
     v
  ntdll.dll                                              Meridian's own
     |  syscall
     v
  Meridian kernel
```

The CPU runs the program's code directly, which is where native speed comes
from. What Meridian has to provide is the behaviour of every function a
program calls.

## Clean room

Behaviour comes only from Microsoft's public documentation, public
specifications such as PE/COFF, and black-box tests on real Windows.
Nobody working on Meridian reads Windows source code, leaked copies
included, nor Wine or ReactOS ([ADR 0004](../adr/0004-clean-room.md)).
Where the documentation is silent, a test program run on real Windows
decides.

## Decisions

| Question | Decision |
| --- | --- |
| 64-bit or 32-bit | both from the start: 32-bit programs run in the CPU's compatibility mode, and every DLL is built twice |
| A function Meridian lacks | the program starts; calling the function stops it with a plain message naming the DLL and function, with details for power users. Nothing pretends to work |
| Coverage before running | `exe-check program.exe` lists which imports Meridian provides |
| Microsoft's own DLLs | never loaded by default, never shipped; using DLLs from a Windows install may become an option |
| Version reported | Windows 10 22H2, 10.0.19045, the version almost every program accepts |
| Registry | a real hierarchical store on disk, with a native tool to browse it |
| `C:\` | `/Windows`, with `C:\Users\<name>` linked to the real home folder |
| Look of standard controls | drawn in Meridian's style |
| Menus | inside the window; the global menu bar is an experiment |
| Installers | run inside a Strata snapshot, so one click undoes them |
| .NET | Microsoft's own open-source .NET runtime, running as a Win32 program |
| DirectX | after a GPU driver exists: Direct3D 11 on Meridian's GPU interface |
| Kernel drivers, anti-cheat | not a goal; they cannot work |

## Order of programs

1. Meridian's own console test programs.
2. Command-line tools built with MSVC or clang-cl against the Universal CRT.
3. PuTTY and 7-Zip.
4. The official `python.exe`, without changes.
5. Microsoft Word, the long-term goal, years away.

## Testing

Each test is a small program in `tests/win32/`, built with `clang-cl` and
`lld-link` (no MSVC, no MinGW). CI runs it on GitHub's Windows runners and
stores the output as the reference; Meridian has to produce the same
output. See [testing.md](../testing.md).
