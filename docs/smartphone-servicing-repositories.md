# Smartphone servicing repository shortlist

Researched 2026-10-30. Target: a Windows desktop application for owner-authorized smartphone servicing, with Android support first and iOS later. This is a starting dependency map, not a claim that one tool can unlock every phone. Exact dependencies depend on phone models and whether “unlock” means bootloader, SIM/carrier, or account recovery.

## Recommended Android-first foundation

| Component | Source | Intended role and constraints | License / evidence |
|---|---|---|---|
| Android SDK Platform-Tools | [Official distribution and release notes](https://developer.android.com/tools/releases/platform-tools) | Use official adb/fastboot binaries initially; Windows, macOS, Linux downloads. Avoid maintaining a private fork before there is a need. | Distribution has its own terms; do not equate source licensing with unrestricted binary redistribution. Pin a tested release during implementation. |
| AOSP adb | [Source repository](https://android.googlesource.com/platform/packages/modules/adb/) | Device discovery, authorized debugging, information and logs. | [Apache-2.0 notice](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/NOTICE); review bundled dependencies when packaging. |
| AOSP fastboot | [system/core repository, fastboot directory](https://android.googlesource.com/platform/system/core/+/refs/heads/main/fastboot/) | Bootloader communication and supported firmware workflows. It is a directory within system/core, not a separate repository. | Directory includes LICENSE; exact license contents were not retrievable in this research pass. |
| scrcpy, optional | [Genymobile/scrcpy](https://github.com/Genymobile/scrcpy) | Android display/control for an authorized device. Normal mirroring needs USB debugging; Android 5+ minimum. | Apache-2.0. Pin a tested release during implementation. |
| Desktop shell, optional | [tauri-apps/tauri](https://github.com/tauri-apps/tauri) | Suggested GUI shell around adapters, device inventory, progress and logs. An implementation choice, not an unlocking dependency. | MIT / Apache-2.0. |

Official Android bootloader unlocking depends on device support, OEM-unlocking permission and on-device confirmation, and resets user data. It is distinct from removing a screen PIN. See [AOSP locking/unlocking requirements](https://source.android.com/docs/core/architecture/bootloader/locking_unlocking).

## Optional device-family adapters

| Repository | Role | License and limits |
|---|---|---|
| [libusb/libusb](https://github.com/libusb/libusb) | Portable USB access for adapters needing direct USB. | LGPL-2.1-or-later. Not automatically needed when invoking existing CLI tools. |
| [Benjamin-Dobell/Heimdall](https://github.com/Benjamin-Dobell/Heimdall) | Samsung firmware flashing using its download-mode protocol; uses libusb. | MIT. Current model compatibility needs testing; its README includes old host OS references, and retrieved page did not expose a latest release date. Treat as experimental until validated against selected models. |
| [libimobiledevice/libimobiledevice](https://github.com/libimobiledevice/libimobiledevice) | iOS information, diagnostics, backups and device services. | README states LGPL-2.1; repository also contains GPL notices, so audit the exact shipped files. No jailbreak required; not an Apple-approved project. |
| [libimobiledevice/libusbmuxd](https://github.com/libimobiledevice/libusbmuxd) | Client interface to the iOS USB multiplexing service. | Library LGPL-2.1; iproxy/inetcat utilities GPL-2.0. Requires a daemon; macOS supplies one, Windows can use AppleMobileDeviceSupport. |
| [libimobiledevice/usbmuxd](https://github.com/libimobiledevice/usbmuxd) | Open-source multiplexing daemon, particularly relevant on Linux. | Repository includes GPL-2.0 and GPL-3.0 license files; check component-level terms. |
| [libimobiledevice/idevicerestore](https://github.com/libimobiledevice/idevicerestore) | Restore/update official iOS firmware. | LGPL-3.0. Firmware signing availability constrains restore/downgrade; restore can erase data. Scope this adapter to official restore workflows. |

The iOS source-build dependency family also includes [libplist](https://github.com/libimobiledevice/libplist) (property lists), [libimobiledevice-glue](https://github.com/libimobiledevice/libimobiledevice-glue) (shared helpers), [libirecovery](https://github.com/libimobiledevice/libirecovery) (recovery-mode USB), and [libtatsu](https://github.com/libimobiledevice/libtatsu) (Apple signing-server communication). Read the selected revision's build manifests for the complete transitive set, including TLS, curl, zip and compression libraries; these four links are not a complete binary packaging manifest. Their individual license audits remain pending.

Apple explicitly says only the carrier can carrier-unlock an iPhone: [Apple carrier-unlock support](https://support.apple.com/en-us/109316). No repository above should be presented as a carrier or Activation Lock bypass.

## Proposed implementation scope

Start with discovery, serial/model identification, connection-state checks, authorized logs and an official bootloader-unlock eligibility workflow. Use explicit per-model capabilities; add firmware flashing only after validating firmware identity and the device state. Add Samsung and iOS separately after a test-device matrix exists. Keep raw USB and destructive operations behind narrow adapters rather than exposing arbitrary shell commands in the UI.

## Concrete build order

Implementation has started in [Phone Studio](../README.md).
The initial release uses a Rust + Slint native desktop
stack in place of the optional Tauri shell. It implements Android discovery,
diagnostics, report export, confirmed reboots and the standard official Fastboot
unlock request with live permission checks. Samsung/iOS adapters remain planned;
no physical-device compatibility has been certified.

1. Windows desktop shell: Tauri with a web UI and a Rust process runner. Build device discovery, connection status, model information, logs, task history and cancel handling around official Platform-Tools. This is the smallest useful starting set: our app plus Tauri and Platform-Tools. Direct libusb integration and scrcpy can wait.
2. Establish a test matrix using specific Pixel models that support the official unlock workflow. Record exact model, OS build, driver, device state and each validated operation. Treat untested models as unsupported for writes.
3. Add guided official bootloader unlocking and approved firmware restoration. Show the actual device, firmware match, expected data loss and required on-device confirmation before a write. Android's official unlock flow performs a factory reset; it does not recover an existing screen PIN.
4. Evaluate Heimdall for individual Samsung models. Add Xiaomi, Motorola, OnePlus and other families through separately documented official workflows after checking each model. A shared Android brand label is insufficient evidence of unlock or flash support.
5. Add the libimobiledevice family for iPhone discovery, authorized device services and signed-firmware restore. Resolve and lock its transitive build dependencies separately for Windows.
6. Package tested versions with dependency notices, verified downloads, a signed installer and an update mechanism. Add account/billing services only if the product will be sold; they are not phone-communication dependencies.

## What repositories do not supply

The product still needs its own device-support database, firmware validation, driver handling, failure recovery, progress reporting and physical-device testing. Manufacturer approval and carrier service workflows cannot be inferred from a GitHub project. Apple says only the carrier can carrier-unlock an iPhone, and Activation Lock recovery uses the owner's credentials or Apple's support process with proof of purchase: [Apple Activation Lock support](https://support.apple.com/en-us/108934).

No complete all-brands dependency manifest can be established before model and operation support is validated. This shortlist is sufficient to start the Android foundation and identifies the main later iOS dependency family. It does not establish compatibility with every device or feature advertised by commercial servicing tools.

Maintenance evidence is limited: latest commit dates for most GitHub projects were not returned by the retrieved pages. README capabilities are upstream claims, not tests performed here. Pin and test chosen versions before shipping. No repositories or phone tools were downloaded or executed during this research.
