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
| [0016](0016-give-the-shell-a-language-of-its-own-and-a-posix-sh-mode.md) | Give the shell a language of its own and a POSIX sh mode | accepted |
| [0017](0017-give-programs-standard-input-and-output-over-the-channel-they-start-with.md) | Give programs standard input and output over the channel they start with | accepted |
| [0018](0018-hand-a-program-its-arguments-as-start-messages.md) | Hand a program its arguments as start messages | accepted |
| [0019](0019-let-a-handle-be-duplicated-and-close-an-object-with-its-last-handle.md) | Let a handle be duplicated, and close an object with its last handle | accepted |
| [0020](0020-keep-a-program-s-working-directory-in-its-runtime-handed-over-at-its-start.md) | Keep a program's working directory in its runtime, handed over at its start | accepted |
| [0021](0021-run-sh-scripts-in-lib-shell-on-the-same-machine-as-the-shell-s-own-language.md) | Run sh scripts in lib/shell on the same machine as the shell's own language | accepted |
| [0022](0022-let-a-program-ask-for-a-bigger-stack-in-a-note-of-its-file.md) | Let a program ask for a bigger stack in a note of its file | accepted |
| [0023](0023-match-posix-regular-expressions-in-lib-regex-with-a-pike-vm.md) | Match POSIX regular expressions in lib/regex with a Pike VM | accepted |
| [0024](0024-run-the-shell-s-shared-commands-as-programs-of-their-own-too.md) | Run the shell's shared commands as programs of their own too | accepted |
| [0025](0025-find-differences-in-lib-diff-with-myers-algorithm-in-linear-space.md) | Find differences in lib/diff with Myers' algorithm in linear space | accepted |
| [0026](0026-run-awk-programs-in-lib-awk-on-a-tree-of-their-own.md) | Run awk programs in lib/awk on a tree of their own | accepted |
| [0027](0027-work-out-bc-s-numbers-exactly-in-lib-bc.md) | Work out bc's numbers exactly in lib/bc | accepted |
