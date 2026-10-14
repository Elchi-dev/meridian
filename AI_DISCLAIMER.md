# How Meridian is made

The code, tests and documentation in this repository are written by an AI model, Claude by Anthropic, working under my direction. I decide what Meridian is, what it does and does not do, and every trade-off written down in `docs/adr/`. The model writes the code.

## Why this file exists

On my other projects I do not talk about the tools I use. Meridian is different. An operating system sits underneath everything you do on a computer: your files, your passwords, your work. You have to trust it more than any app. Running it without knowing how it was made would be unfair to you, and finding out later would break that trust. So I am saying it up front.

## How the work is checked

Being written by a model does not lower the bar. If anything it raises it, so the rules are built into the tooling and CI instead of relying on anyone's discipline:

- Every component has at least 1.5 lines of test code for every line of real code, counted without comments or blank lines. `sextant stats --check` fails the build otherwise.
- Tests have to be able to fail. `sextant mutate` changes the code on purpose (flips a comparison, swaps a return value, deletes a statement) and every change that no test notices fails CI unless it has a written reason.
- No stubs, no placeholder functions, no fake mocks. Hardware code is tested against the emulated hardware in QEMU, Windows behaviour against real Windows.
- No outside code. Meridian has zero dependencies; the few outside files it needs (fonts, firmware, the Secure Boot shim) each have their own decision record explaining why.
- Every decision is written down with the alternatives that were rejected and why.

## Windows compatibility and clean room

Meridian runs Windows programs, so it has to behave like Windows. How it learns that behaviour is a hard rule:

- Allowed: Microsoft's public documentation, public specifications such as PE/COFF, and black-box tests that run small programs on real Windows and record what happens.
- Never allowed: reading the source code of Windows (including any leaked copy), Wine, ReactOS or any similar project. The model is instructed never to open them, and this rule is part of every task it gets.

There is one thing I cannot control. Nobody outside Anthropic can see what the model was trained on. If code from Wine or another project was part of that training data, I cannot rule out that something similar ends up here, and I cannot check for it. If you find code in Meridian that looks copied from somewhere, please tell me (see `SECURITY.md`) and it will be rewritten from the documentation.

Made by Elchi
