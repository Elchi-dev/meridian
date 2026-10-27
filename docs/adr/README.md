# Decision records

Why Meridian is the way it is. Each record says what was decided, what else was on the table and why it lost. Start a new one with `sextant adr new "title"`.

Outside code and files are only allowed through an accepted record with an `- Allows:` line naming them. `sextant lint` fails on anything that is not.

| No. | Decision | Status |
| --- | --- | --- |
| [0001](0001-record-decisions.md) | Record every decision that shapes Meridian | accepted |
| [0002](0002-dual-licence.md) | License Meridian under MIT or Apache-2.0 | accepted |
| [0003](0003-zero-dependencies.md) | Take no outside code, and justify every outside file | accepted |
| [0004](0004-clean-room.md) | Learn Windows behaviour only from public documentation and black-box tests | accepted |
| [0005](0005-meri-format.md) | Write settings and system files in .meri | accepted |
| [0006](0006-rust-stable-pinned.md) | Build with stable Rust, pinned to one exact version | accepted |
| [0007](0007-assembly-in-rust.md) | Write assembly inside Rust, in Intel syntax | accepted |
| [0008](0008-unsafe-in-raw-modules.md) | Keep all unsafe code in raw modules | accepted |
| [0009](0009-testing-rules.md) | Test every component with real code, and prove the tests can fail | accepted |
| [0010](0010-sextant-writes-disk-images.md) | Write disk images with sextant itself | accepted |
| [0011](0011-ci-checkout-only.md) | Run CI on GitHub Actions with actions/checkout as the only action | accepted |
| [0012](0012-fonts.md) | Use Inter and JetBrains Mono as the system fonts | accepted |
| [0013](0013-start-through-a-shim-signed-by-microsoft.md) | Start through a shim signed by Microsoft | accepted |
| [0014](0014-give-every-process-a-security-token-in-the-shape-of-windows-own.md) | Give every process a security token in the shape of Windows' own | accepted |
| [0015](0015-write-only-to-partitions-meridian-made-and-to-others-once-unlocked.md) | Write only to partitions Meridian made, and to others once unlocked | accepted |
