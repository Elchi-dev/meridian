# 0011. Run CI on GitHub Actions with actions/checkout as the only action

- Status: accepted
- Date: 2026-10-14
- Allows: actions/checkout

## Context

Every push needs the same checks on a clean machine: lint, the test ratio, formatting, clippy, the host tests, and booting the test image in QEMU. GitHub Actions is where the repository lives. Actions from the marketplace are code from outside like any crate (0003), and most of them only wrap a shell command.

## Decision

CI uses exactly one outside action, `actions/checkout`, pinned to the full commit hash of v7.0.1 so a moved tag cannot change what runs. Everything else is plain shell steps:

- Rust comes from the `rustup` already on the runners, which installs the version `rust-toolchain.toml` pins.
- Go comes from the runner's Go, which fetches the version `go.mod` asks for (`GOTOOLCHAIN=auto`).
- QEMU, OVMF and the cross-check tools (`dosfstools`, `mtools`, `xorriso`, `fdisk`, and `qrencode` and `zbarimg` for the QR codes) come from Ubuntu's packages. `openssl`, which checks sextant's Authenticode signatures, is already on the runners. Tests only run them to check Meridian's own output; nothing Meridian builds contains or calls them.

Linux runs every check on every push, with KVM enabled for QEMU and `SEXTANT_REQUIRE_TOOLS=1`, so a missing tool fails instead of skipping tests. After those pass, a second Linux job mutates the files the push changed, with the QEMU checks for crates that have them. macOS and Windows build and test sextant on pushes to `main` and once a week, because sextant has to work on every host Samuel might build on.

## Alternatives considered

- **`actions/setup-go`, `dtolnay/rust-toolchain`, `actions/cache`.** Convenient, faster on cache hits, and three more pieces of outside code with their own updates. The runners already carry both toolchains.
- **No action at all, a manual `git clone`.** Possible, but checkout also handles authentication, the exact commit of a pull request and sparse fetches correctly, and doing that by hand is more fragile than one pinned, widely read action.
- **A self-hosted runner on Samuel's machine.** Faster QEMU, but CI would then depend on one desktop being on, and its results would depend on whatever else that machine runs.

## Consequences

CI starts a little slower without caches. The macOS runners keep their Go only in the tool cache, so a shell step puts it on the path, and installs Go with Homebrew if the cache ever has none. Updating `actions/checkout` is a deliberate change to the pinned hash in `.github/workflows/ci.yml` and to this record.
