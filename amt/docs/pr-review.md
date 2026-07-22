# `sdague/amt` PR Review

Source repository: <https://github.com/sdague/amt>

Reviewed refs:

- `master`: `343d19dd9908d07fc8cc6890745a3bae44c8c840`
- PR #19: `c557c88898fea1aaeb06dff23a8b2b4132b95521`
- PR #29: `80cf55d7fbe9b3af4a27d97874acddf99a37d857`

## PR #19

PR #19 is not a good base for the Rust port. It removes useful 0.8.0 behavior, drops hibernate support, reverts host database improvements, and introduces Python 2 style exception output in code that had already moved toward newer Python compatibility.

Accepted idea:

- Add request timeout handling. The Rust client uses a 10 second HTTP timeout.

Rejected changes:

- Removing hibernate support.
- Reverting version, history, and test hygiene.
- Using the AMT password as a VNC password. This project has no VNC support.

## PR #29

PR #29 is the useful modern protocol reference. It adds HTTPS transport support, uses `/wsman` as the WS-Addressing `To` value, introduces UUID and version lookups, and expands WSMAN helpers beyond the original power-only scope.

Accepted changes:

- HTTP and HTTPS AMT transport on ports `16992` and `16993`.
- WSMAN endpoint path `/wsman`.
- WS-Addressing `To` value `/wsman` for current AMT compatibility.
- AMT UUID lookup from `CIM_ComputerSystemPackage`.
- AMT firmware version from the HTTP `Server` header.
- Hard reset power state code `10`.
- Separate host registry management from device commands.

Rejected or deferred changes:

- VNC and KVM commands remain excluded.
- PKI and AMT TLS mutation commands are deferred. They are useful but outside the requested simple power and boot scope and carry higher lockout risk.
- XML mutation is kept small and command-specific rather than porting the full Python helper surface.
