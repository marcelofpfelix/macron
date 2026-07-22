# AMT Protocol Scope

This project targets Intel AMT devices that expose the WSMAN/CIM interface. The older SOAP/EOI interface used by `amttool` is not implemented.

## Transport

- HTTP: `http://<host>:16992/wsman`
- HTTPS: `https://<host>:16993/wsman`
- Authentication: HTTP Digest
- SOAP content type: `application/soap+xml;charset=UTF-8`
- WS-Addressing `To`: `/wsman`

The `/wsman` `To` value follows the compatibility change from `sdague/amt` PR #29. The original Python release used the full URI in that header.

## Implemented CIM Operations

- `CIM_PowerManagementService/RequestPowerStateChange`
- `CIM_AssociatedPowerManagementService` power status lookup
- `CIM_BootConfigSetting/ChangeBootOrder`
- `CIM_BootService/SetBootConfigRole`
- `CIM_ComputerSystemPackage` platform UUID lookup

## Supported Power States

- `on` -> `2`
- `sleep` -> `4`
- `reboot` -> `5`
- `hibernate` -> `7`
- `off` -> `8`
- `reset` -> `10`

## Explicit Non-Goals

- VNC or KVM enablement and status.
- AMT PKI inventory and mutation.
- AMT TLS certificate installation or TLS policy mutation.
- Fleet management, discovery, or remote relay support.
