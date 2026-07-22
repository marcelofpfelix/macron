# Changelog

## 0.1.0 - 2026-06-16

- Start the Rust port of the non-VNC parts of `sdague/amt`.
- Add simple AMT power commands: on, off, reboot, reset, sleep, hibernate, and status.
- Add one-shot boot target commands for PXE, hard drive, and CD/DVD.
- Add `pxeboot` for set-PXE-next-boot plus reboot.
- Add AMT UUID and firmware `Server` header lookup.
- Use WSMAN/CIM over HTTP Digest with HTTP and HTTPS AMT ports.
- Use `/wsman` as the WS-Addressing target for current AMT compatibility.
- Add `password_ref` support for `gopass:<path>` secrets.
- Add repo-local `.config/amt/hosts.toml` support while keeping `.config/` ignored.
- Add `make compile`; make `make install` copy the release binary to `~/bin`, with `install-bin` kept as an alias.
- Add roadmap documentation for inventory, BIOS/preboot, PXE install workflows, SOL, IDE-R, and sensors.
- Exclude VNC/KVM and unproven remote-install commands from the CLI.
