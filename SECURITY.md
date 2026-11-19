# Security Policy

## Supported versions

The latest published release is supported with security fixes. Older releases may
not receive fixes; please upgrade before reporting an issue that is already fixed.

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Use GitHub's private
security advisory flow:

<https://github.com/Maythelos/LanPaster/security/advisories/new>

Include as much detail as possible:

- affected version and platform
- steps to reproduce
- impact (for example: clipboard disclosure, file overwrite, path traversal,
  authentication bypass)
- any logs or a proof of concept, with secrets removed

You will receive an acknowledgement when the report is reviewed. There is no bug
bounty program.

## Security model

LanPaster is a LAN-trusted tool, not a hardened secure messenger:

- Clipboard data is sent directly to peers over a Noise `XX` encrypted TCP
  channel. There is no cloud service or relay.
- Device identities are persistent X25519 key pairs. The first connection must be
  approved from the tray; trusted peers are stored in a local
  `*.trusted.toml` file.
- Removed peers are recorded as revoked and are not silently re-paired by mDNS.
- On Unix, config and trusted files are written with mode `0600`; received file
  directories use `0700`.
- Received files are not opened automatically and do not keep executable or
  setuid bits.

Known limitations:

- No OS keychain integration, certificate pinning, signed audit log, or
  protection against a compromised local account.
- Clipboard history is kept in memory as plaintext.
- Windows and Linux code paths have not yet received as much real-device
  validation as macOS arm64.

Do not use LanPaster to transfer material whose compromise would be catastrophic
on a network you do not trust.
