# Roadmap

This roadmap separates quick AMT power/boot improvements from larger management-console features. The default project direction remains small and scriptable.

## Quick Wins

- `install-bin` Makefile target: release build and copy to `~/bin`. Implemented.
- `wait` helpers: poll power status after power or install operations until the AMT response changes or a timeout expires.
- `net info`: show AMT management address, MAC, DHCP/static mode, hostname/FQDN, and link details where exposed.
- `hw info`: show platform UUID, vendor, model, BIOS version, CPU, and memory inventory where exposed.
- `status --json`: machine-readable status for scripts.
- Safer host registry output: add JSON output and masked password diagnostics.

## Network And Inventory

- AMT network identity: query AMT-exposed network settings for management IP, MAC address, DHCP/static mode, hostname/FQDN, and link state.
- Host OS address caveat: document that AMT does not reliably know every IP configured inside the running OS; DHCP leases, DNS, ARP/neighbor tables, SSH, or an OS agent are better sources for OS-level addresses.
- Hardware inventory: expose AMT/DASH inventory such as platform UUID, manufacturer, model, BIOS version, CPU, memory, baseboard, and chassis where available.
- Firmware inventory: expand `version` into a structured command that can include AMT firmware, BIOS, and platform identifiers.
- Sensors: best-effort temperature, fan, and voltage reporting if the OEM firmware exposes sensor classes; return a clear unsupported result otherwise.

## Remote OS Installation

- `install pxe` CLI alias: defer until the PXE workflow is proven against real hardware and install infrastructure.
- PXE workflow wrapper: validate host reachability, set PXE next boot, reboot, then optionally poll status.
- Installer notes: document expected DHCP/TFTP/HTTP/PXE infrastructure rather than embedding a provisioning server in this CLI.
- Boot target reset helper: set next boot back to disk after install flows where firmware does not auto-reset as expected.

## BIOS And Preboot

- Boot-device inventory: query available AMT boot sources before setting them.
- BIOS setting inventory: read supported BIOS/platform settings where exposed by AMT/DASH classes.
- BIOS setting mutation: possible, but should be gated behind explicit confirmations because bad settings can lock out remote access.

## Console And Media Redirection

- SOL support: possible future text-console path for systems with usable serial console redirection.
- IDE-R/media redirection: possible future path for mounting remote install media on supported hardware.
- KVM/VNC console support: intentionally not in this project scope right now. AMT KVM is hardware/OEM dependent and would add a larger interactive surface.

## Security And Provisioning

- TLS policy inspection: read AMT TLS configuration without mutating it.
- Certificate inventory: list AMT certificates and keys without adding/removing them.
- TLS/PKI mutation: defer unless there is a concrete need; misconfiguration can lock out management access.
