# 0013. Start through a shim signed by Microsoft

- Status: accepted
- Date: 2026-10-15
- Allows: third-party/shim/shimx64.efi, third-party/shim/mmx64.efi, third-party/shim/COPYRIGHT

## Context

Secure Boot is on by default on almost every PC sold since Windows 8. The firmware starts only what a key in its `db` signed, and on nearly every machine those keys are Microsoft's: one for Windows, and the Microsoft Corporation UEFI CA 2011 for everyone else, joined by its 2023 successor on newer machines. A loader signed with Meridian's key alone does not start until the person enrolls that key in the firmware's setup or turns Secure Boot off. Samuel wants neither (QUESTIONS.md 8.4): no firmware settings, and for that one binary signed by Microsoft, either a shim or the Linux Foundation's PreLoader, with its own record.

A first stage of Meridian's own, signed by Microsoft, would need shim review, a company behind it and an EV code signing certificate for Microsoft's hardware portal, about 600 USD a year. That is too early for a project in M2.

What the two candidates are:

- **shim** is BSD-2-Clause and maintained; 15.8 came out at the end of 2023, and every large Linux distribution ships one signed by the Microsoft UEFI CA. It checks the next stage against `db`, against a certificate built into it, and against the Machine Owner Key list (MOK). When a check fails, it starts MokManager, where the person can enroll a key from a file on the disk. It also enforces SBAT: the next stage needs a `.sbat` section, which `meridian.efi` has had since M1.
- **PreLoader**'s signed build dates from 2013, and its source is part of efitools, under GPL-2.0. It has no SBAT, and it trusts only hashes enrolled with HashTool, so every new build of `meridian.efi` would have to be enrolled again.

## Decision

Meridian starts through Ubuntu's signed shim, shipped unchanged under `third-party/shim/`. The files come from Ubuntu 24.04's package `shim-signed` 1.58+15.8-0ubuntu1 (`pool/main/s/shim-signed/shim-signed_1.58+15.8-0ubuntu1_amd64.deb`, SHA-256 `ba9b5d80e5d886c30664f2bebfb5c2fcce3b9b40f16fc46cba49c19a91c8059c`, which matches the archive's signed index). On 15 October 2026 it is the newest version in both noble and noble-updates.

| File | From the package | SHA-256 | Signed by |
| --- | --- | --- | --- |
| `shimx64.efi` | `usr/lib/shim/shimx64.efi.signed.latest` | `6fe6e1bcbe6cf6baec8e056d40361ca1aa715cc04ddcc2855351de060b84350b` | Microsoft Corporation UEFI CA 2011 |
| `mmx64.efi` | `usr/lib/shim/mmx64.efi` | `d2fa8e52fddc99dad94a0009fee23cb2478c28373b777d50b2f784eb4e96f88e` | Canonical Ltd. Secure Boot Signing (2022 v1), which the shim trusts through the certificate built into it |
| `COPYRIGHT` | `debian/copyright` of the source package `shim` 15.8-0ubuntu1 (`shim_15.8-0ubuntu1.debian.tar.xz`, SHA-256 `15ed98489d54aa882aab5977dfc654e62c14c6bd5f72c785d4a34e2cbb1099b4`) | `278ea11f13eb4623b8d0d574262fc77dd53d77fdcdb6c30030fe049ed260d43d` | |

The signed package's own copyright file only names shim's BSD-2-Clause, so `COPYRIGHT` is the one of the source package the binaries were built from. It lists every licence in them: BSD-2-Clause for shim, the OpenSSL and original SSLeay licences for the OpenSSL inside, BSD-2-Clause-Patent for edk2's parts, and a few files under BSD-3-Clause or in the public domain. That source package's tarball is byte for byte upstream's `shim-15.8.tar.bz2` (SHA-256 `a79f0a9b89f3681ab384865b1a46ab3f79d88b11b4ca59aa040ab03fffae80a9`).

On the EFI system partition the files go where the firmware and the shim look for them:

- `EFI/BOOT/BOOTX64.EFI` is the shim, the file the firmware starts from a removable disk.
- `EFI/BOOT/grubx64.efi` is `meridian.efi`. The shim starts the file with that name next to itself, and the name is built into the binary.
- `EFI/BOOT/mmx64.efi` is MokManager.
- `meridian.cer` is the certificate of the key that signed the loader, for enrolling. It lies at the top of the partition, so it is the first thing MokManager shows once the person picks the disk.
- `meridian/licences/shim.txt` is `COPYRIGHT`, since the licences ask for their notices to go along with the binaries.

On the first boot the shim cannot check `meridian.efi` yet and starts MokManager. The person picks "Enroll key from disk", chooses `meridian.cer` and confirms. After the restart every loader signed with that key starts without a question. Release images carry the release key's certificate, development images the key sextant makes for each machine.

Meridian's own first stage, reviewed and signed by Microsoft, stays the long-term goal. Samuel will pay for the certificate once Meridian has people using it (15 October 2026), and then this record is replaced.

## Alternatives considered

- **PreLoader.** Every image would have to come with the GPL source of efitools or a written offer of it. Its signed build is from 2013, and it has no SBAT. Because it trusts hashes, the person would enroll again after every loader update, where a shim trusts a key once.
- **A shim from another distribution, such as Fedora or Debian.** They work the same way. Ubuntu's can be fetched and checked with `apt` on the build container and on the CI runners, which both run Ubuntu.
- **Turning Secure Boot off, or enrolling Meridian's key in the firmware's setup.** Those are exactly the firmware settings Samuel wants to spare people, and every vendor's setup screens differ. The firmware path keeps working, since the shim also trusts `db`, and the existing test of Meridian's own key enrolled from Setup Mode keeps covering it.

## Consequences

- A machine with Secure Boot on starts Meridian without touching the firmware's settings, after one confirmation in MokManager. A later installer can queue the enrollment from inside Meridian, so that confirmation is all that is left.
- `sextant image` writes the shim, MokManager, `meridian.efi` under its fixed name and the certificate. The loader's scan of the EFI partition has to skip `EFI/BOOT`, so it does not offer its own shim as another system.
- The Secure Boot tests gain a run on OVMF with Microsoft's keys: the shim starts, sextant presses through MokManager's enrollment, and after the restart the loader starts. A loader signed with a key that was not enrolled has to be refused.
- The Microsoft Corporation UEFI CA 2011 expired on 27 June 2026, and the certificate that signed this shim already in October 2024. edk2-based firmware, which most PCs use, does not check certificate dates when it checks an image, so the signature keeps working wherever `db` holds that CA. For machines that only trust the 2023 CA, and whenever Ubuntu ships a newer shim for 24.04, the files and this record's table are replaced in one commit.
- An SBAT or `dbx` update that revokes shim 15.8 stops it on machines that received the update. Watching Ubuntu's `shim-signed` is now part of keeping Meridian bootable.
- The chain of trust still ends at `meridian.efi` until the loader checks the kernel's signature, as the architecture document says.
