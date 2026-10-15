# Security

## Supported versions

None yet. Meridian is at version 0.x and has no releases. Fixes go to `dev`
and then `main`.

## Reporting a vulnerability

Please do not report security problems in public pull requests or
discussions.

- Once the repository is public, use GitHub's private vulnerability
  reporting: the "Report a vulnerability" button on the repository's
  Security tab. Only the maintainer sees the report.
- Until then, contact the maintainer through GitHub,
  [@Elchi-dev](https://github.com/Elchi-dev), and ask for a private channel
  before sending details.

Include what you found, how to reproduce it, and what it lets an attacker
do. You will get an answer within a week. Fixes are credited unless you ask
not to be named.

## What is in scope

Everything in this repository: the loader, the kernel, the Windows
personality, Strata, Horizon and sextant. Reports about code that may have
been copied from another project are also welcome (see the clean-room rule
in [ADR 0004](docs/adr/0004-clean-room.md)); that code will be rewritten.

## The release key

Release images are signed with Meridian's release key, and its certificate
is the `meridian.cer` a person enrolls in MokManager on the first start.
The certificate is [`keys/release.cer`](keys/release.cer), for code signing,
valid until 2056, with this SHA-256:

`4b232965d95aefec9d7f32b42a5798e2c8b8516268830537faa1ac2a0418b4f0`

The key itself is only in CI's `RELEASE_KEY` secret and in an offline
backup. Development images are signed with a key each machine makes for
itself, so a key enrolled for a development image does not start release
images, and the other way round.

## Known limits

Meridian's kernel does almost nothing yet, so most of its security model
is design (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)). What exists
already holds to it: the kernel runs with no-execute pages, write
protection, nothing mapped both writable and executable, a guard page
under its stack and nothing mapped at address 0. Two limits are known and
written down:

- **The chain of trust ends at the loader.** With Secure Boot on, the
  firmware checks the shim and the shim checks `meridian.efi`, but the
  loader does not check the kernel's signature yet. That needs Meridian's
  own SHA-256 and signature code.
