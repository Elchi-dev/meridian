# Decision records

Why Meridian is the way it is. Each record says what was decided, what else was on the table and why it lost. Start a new one with `sextant adr new "title"`.

Outside code and files are only allowed through an accepted record with an `- Allows:` line naming them. `sextant lint` fails on anything that is not.

| No. | Decision | Status |
| --- | --- | --- |
| [0001](0001-record-decisions.md) | Record every decision that shapes Meridian | accepted |
| [0002](0002-dual-licence.md) | License Meridian under MIT or Apache-2.0 | accepted |
| [0003](0003-zero-dependencies.md) | Take no outside code, and justify every outside file | accepted |
| [0004](0004-clean-room.md) | Learn Windows behaviour only from public documentation and black-box tests | accepted |
| [0009](0009-testing-rules.md) | Test every component with real code, and prove the tests can fail | accepted |
| [0006](0006-rust-stable-pinned.md) | Build with stable Rust, pinned to one exact version | accepted |
