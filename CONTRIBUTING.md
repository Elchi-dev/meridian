# Contributing

Thanks for looking. Meridian has strict rules, and most of them are
enforced by tools, so a pull request learns quickly whether it fits.

## Setting up

You need:

- Go 1.27 or newer
- rustup, which installs the pinned Rust 1.99.0 from `rust-toolchain.toml` by itself
- QEMU and OVMF for booting

```
cd tools/sextant && go build -o sextant ./cmd/sextant
./sextant doctor          # checks the tools and installs what is missing
```

## Before every commit

```
./sextant ci
```

It runs exactly what CI runs, in the same order, and stops at the first
check that fails: the project rules, the test lines, formatting, clippy for
every target, `go vet`, the Go tests with the race detector, the Rust tests
and the QEMU test image. CI's linux job runs the same command, so the two
cannot drift apart. `./sextant ci --list` shows the checks.
[docs/testing.md](docs/testing.md) covers QEMU tests and mutation testing.

## The rules

- **No outside code.** No crates, no Go modules, no code copied from other projects. Outside files (fonts, firmware, the Secure Boot shim) need an accepted decision record with an `- Allows:` line before they land ([ADR 0003](docs/adr/0003-zero-dependencies.md)). Driver code may only be translated from MIT, BSD or ISC licensed sources, never GPL.
- **Clean room.** Never read Windows source code (leaked copies included), Wine or ReactOS, and never contribute anything derived from them. Windows behaviour comes from Microsoft's public documentation and from test programs run on real Windows ([ADR 0004](docs/adr/0004-clean-room.md)).
- **`unsafe` only in `raw` modules** (`raw.rs` or `raw/`). Every block gets a `// SAFETY:` comment that says why it holds (ADR 0008).
- **Assembly inside Rust**, Intel syntax, through `asm!`, `global_asm!` and naked functions (ADR 0007).
- **Tests.** At least 1.5 lines of test code per line of code in every component, counted without comments and blank lines. Every mutant `sextant mutate` creates must be killed, or explained in `mutants.meri` ([ADR 0009](docs/adr/0009-testing-rules.md)).
- **No mocks and no stubs.** Test hardware code in QEMU. A function either does what its name says, or returns a documented error for what it does not support.
- **No dashes.** No em dashes or en dashes anywhere, in code, docs or commit messages. Use a comma, a colon or a full stop, and a hyphen in ranges.
- **No placeholders.** No `TODO` or `FIXME` comments, no `todo!()`. Open work goes into [TODO.md](TODO.md).
- **Comments** explain why, never what, and only where the code cannot say it.
- **Decisions** that change how Meridian is built or behaves get a record in `docs/adr/`.

## Commits and branches

- **Commit messages.** Conventional, with a scope where it helps: `feat(uefi): read the memory map`, `fix(sextant): ...`, `test(meri): ...`, `docs: ...`. Imperative, a subject under about 70 characters, and a body that says why when it is not obvious.
- **Small commits.** One logical step per commit, each building on its own.
- **Branches.** Work happens on `dev`, and pull requests target `dev`. CI may fail on `dev`. `main` only ever receives commits whose CI passed.

## Reporting problems

GitHub issues are off; [TODO.md](TODO.md) is the list of open work. For a
bug, open a pull request that adds a failing test, or start a discussion in
a pull request. For anything security related, see [SECURITY.md](SECURITY.md).

## Licence of contributions

Unless you say otherwise, anything you submit for inclusion is licensed
under MIT or Apache-2.0, at the user's choice, like the rest of Meridian,
without additional terms.
