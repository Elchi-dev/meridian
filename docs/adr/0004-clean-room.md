# 0004. Learn Windows behaviour only from public documentation and black-box tests

- Status: accepted
- Date: 2026-10-14

## Context

Meridian runs Windows programs without Windows. That means reimplementing what they call: ntdll, kernel32, user32, gdi32, the C runtime and much more. Other projects have done parts of this. Wine (LGPL) and ReactOS (GPL) are open source, and leaked Windows source code circulates online. Reading any of them would taint Meridian's code legally and make its licence (0002) impossible to defend.

## Decision

Windows behaviour is learned only from:

- Microsoft's public documentation (Microsoft Learn, the Windows SDK headers' documented interfaces, published specifications)
- public specifications such as PE/COFF, Authenticode and the UEFI specification
- black-box tests: small programs in `tests/win32/` run on real Windows in CI, and their recorded output is what Meridian must reproduce

Never, for any reason: Windows source code including leaked copies, Wine, ReactOS, or any other reimplementation of Windows interfaces. This also covers reading their bug trackers for implementation details.

## Alternatives considered

- **Reading Wine or ReactOS for behaviour, without copying.** Common practice, and it would save time. It still exposes the project to claims of derived work, and the line between "understood" and "copied" cannot be proven afterwards.

## Consequences

Where documentation is silent, the only way forward is a test against real Windows, which is slower but also produces a test Meridian keeps forever. `AI_DISCLAIMER.md` states the one part this rule cannot cover: what the model writing the code was trained on.
