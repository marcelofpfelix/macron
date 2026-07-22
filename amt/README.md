# amt

`amt` is a small Rust CLI for Intel Active Management Technology power and boot
control. It is a port of the useful non-VNC parts of
[`sdague/amt`](https://github.com/sdague/amt).

The scope is intentionally narrow:

- power on, off, reboot, reset, sleep, hibernate, and status
- one-shot boot target changes for PXE, hard drive, and CD/DVD
- PXE boot helper
- AMT UUID and firmware `Server` header lookup
- HTTP and HTTPS WSMAN/CIM transport with HTTP Digest authentication

VNC/KVM support is deliberately excluded.

## Install

```sh
make install            # release build, then copy to ~/bin/amt
make install BIN_DIR=/usr/local/bin
make install-cargo      # cargo install --path .
```

## Direct Usage

```sh
amt direct --host 192.0.2.10 --password-ref gopass:homelab/amt power status
amt direct --host 192.0.2.10 --password-ref gopass:homelab/amt power on
amt direct --host 192.0.2.10 --password-ref gopass:homelab/amt pxeboot
amt direct --host 192.0.2.10 --password-ref gopass:homelab/amt --protocol https --accept-invalid-certs uuid
```

AMT defaults:

- HTTP: `16992`
- HTTPS: `16993`
- WSMAN path: `/wsman`
- username: `admin`

Full AMT URLs are accepted. A URL ending in `/` is normalized to `/wsman`.

## Host Registry

The host registry is a convenience for lab machines. Prefer `password_ref`
entries so AMT passwords stay in a password manager instead of plaintext TOML.
The CLI currently supports `gopass:<path>` references, resolved at runtime with
`gopass show -o <path>`.

Config lookup order:

- `AMT_CONFIG`, if set
- repo-local `.config/amt/hosts.toml`, if present
- the user config directory, usually `~/.config/amt/hosts.toml`

```sh
amt host set nuc http://nuc-amt.internal.bandonga.com:16992/ --password-ref gopass:homelab/amt
amt host list
amt host get nuc
amt run nuc power status
amt run nuc boot pxe
```

Print the registry path:

```sh
amt host path
```

## Protocol Notes

The implementation uses WSMAN/CIM, not the older SOAP/EOI interface removed from
AMT 9. The WSA `To` header is sent as `/wsman`, following the compatibility
direction from `sdague/amt` PR #29. HTTPS transport is supported for modern AMT
firmware; `--accept-invalid-certs` is available for lab devices with unmanaged
AMT certificates.

TLS certificate and PKI mutation commands from PR #29 are intentionally out of
scope for this first Rust port. The CLI supports using AMT over HTTPS, but it
does not configure AMT TLS state, install certificates, or manage AMT keys.

## Provenance

This is a Rust port of the non-VNC AMT power and boot control behavior from
[`sdague/amt`](https://github.com/sdague/amt), which is licensed under
Apache-2.0. The Python source was reviewed at `343d19d`, with PR #19 and PR #29
covered in [docs/pr-review.md](docs/pr-review.md). See [docs/roadmap.md](docs/roadmap.md)
for possible future additions.

## Development

```sh
make compile
make check
make install
```

The project follows the local Rust repo conventions from `macron` and
`team-telnyx/tel-proxy-blackdog`: a small Makefile, pre-commit hooks, GitHub
Actions for fmt/clippy/tests, `VERSION`, and `CHANGELOG.md`.
