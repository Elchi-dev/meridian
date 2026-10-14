# 0002. License Meridian under MIT or Apache-2.0

- Status: accepted
- Date: 2026-10-14

## Context

Meridian is public from its first commit. People will read it, run it, and maybe build on it. The licence decides what they may do and which outside code could ever be combined with it.

## Decision

Everything in this repository is available under either the MIT licence (`LICENSE-MIT`) or the Apache License 2.0 (`LICENSE-APACHE`), at the user's choice. Contributions are accepted under the same terms.

## Alternatives considered

- **MIT only.** Shorter, but carries no patent grant from contributors. For an operating system that implements large, well known interfaces, that grant is worth having.
- **GPL-3.0.** Would force anyone shipping a modified Meridian to publish their changes. It also makes Meridian unusable as a base for anyone who cannot accept that, and it is the less common choice in the Rust ecosystem.
- **MPL-2.0.** A reasonable middle ground, but file-level copyleft adds rules for contributors without a clear benefit for a project that takes no outside code anyway (0003).

## Consequences

- GPL code can never be copied into Meridian. Apache-2.0 is incompatible with GPLv2, and pulling GPL code in would turn the combined work into GPL. This rules out porting Linux drivers, which are almost all GPLv2. Driver code may only be studied or translated from sources under MIT, BSD or ISC style licences, with attribution and its own record (0003).
- Both licence files have to ship with every release artefact.
