# 0014. Give every process a security token in the shape of Windows' own

- Status: accepted
- Date: 2026-10-20

## Context

QUESTIONS.md 7.10 settled that every process carries a security token from
the start, that the UI starts with one user, and that sandboxing is
optional, with things like the camera behind a permission the user gives
by hand. The user may do anything on their system, without the fear of
wiping it by accident.

Windows programs read their token all the time: `GetTokenInformation` for
the user and groups, `CheckTokenMembership` for "am I an administrator",
`OpenProcessToken` before `AdjustTokenPrivileges`, and integrity levels
decide what a browser's sandboxed renderer may touch. All of it is
documented publicly in [MS-DTYP] (security identifiers, access masks, the
access check algorithm) and in the Windows API reference, so ADR 0004 lets
Meridian follow it.

Nothing in Meridian checks access by name yet: there are no files and no
named objects before the VFS. What exists are handles with rights, which
stay the way a process reaches an object it already holds.

## Decision

A token is the one Windows has, minus what only domains need:

- the user and up to 16 groups, each a security identifier (SID) in the
  binary and text forms of [MS-DTYP] 2.4.2, so `S-1-5-18` means SYSTEM on
  Meridian as on Windows and Win32 calls hand SIDs out unchanged;
- a set of privileges, Meridian's own bits with a Windows name each, such
  as debugging other programs or shutting the machine down;
- an integrity level, untrusted, low, medium, high or system, with the
  rule that a process cannot write to what is labelled higher than itself.

`lib/security` holds all of it and the access check of [MS-DTYP] 2.5.3.2,
an object's owner and discretionary list of allowed and denied entries
against a token and the rights asked for, without a processor. The kernel
keeps a token with every process. A process started with SPAWN gets its
parent's token, or a restricted copy of it: a lower integrity level and
fewer privileges, never more. Programs read their own token with TOKEN.

Programs the kernel starts at boot run as SYSTEM, with every privilege and
the system integrity level, as Windows' services do. The user's token
comes from logging in, which arrives with init in M5: the one user's
processes run at the medium level with their administrator privileges
off, and turning them on is a choice the user makes on screen, clear about
what it allows, not a password prompt in a terminal.

## Alternatives considered

- **Unix user and group ids.** Small numbers are simpler, but every Win32
  program that asks for its SID would need a made-up translation, and
  integrity levels and privileges would have to be added beside them
  anyway. The Windows shape covers both worlds.
- **Capabilities only, no identity.** Handles already are capabilities,
  and the sandbox will grant more of them. But files on disk need an owner
  and a list of who may do what that outlives any process, and Windows
  programs expect exactly that.
- **Wait for the VFS.** Retrofitting a token into every process later
  touches every way a process starts. Starting with one now costs a few
  hundred bytes per process.

## Consequences

- Every new kind of named object and every file gets a security
  descriptor, checked with `lib/security` when it is opened, and its
  handle then carries the rights granted.
- SIDs need room: the kernel keeps tokens in fixed arrays, so a token
  holds at most 16 groups. Domains with hundreds of groups are not a goal.
- App sandboxing becomes a further kind of restriction on a token, with
  capabilities for the camera, the microphone and the user's files, when
  Horizon can ask the user.
- The privileges, the integrity levels and the access check have to keep
  matching [MS-DTYP] where Windows programs can see them.

[MS-DTYP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/
