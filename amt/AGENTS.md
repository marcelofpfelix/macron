# amt Local Agent Notes

- Keep this project small: AMT power and boot control, not a fleet manager.
- Do not add VNC/KVM features.
- Treat WSMAN/CIM compatibility as the core contract.
- Prefer direct host/password flags for tests and one-off usage; keep the host registry as a convenience only.
- Before claiming completion, run `make check`.
