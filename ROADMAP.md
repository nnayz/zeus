# Roadmap

This is direction, not a promise or release calendar. Concrete work is tracked
in [Issues](https://github.com/nnayz/zeus/issues).

## Now

- Keep `main` green and protected by required CI checks.
- Make session persistence and daemon upgrades boring and recoverable.
- Expand first-class agent manifests and status-detection coverage.
- Improve contributor docs, security reporting, privacy disclosure, and release
  supply-chain checks.

## Next

- Bring the Rust engine to feature parity with the shipped Swift daemon.
- Improve remote-node setup, diagnostics, and least-privilege guidance.
- Add deeper end-to-end tests for app updates and session recovery.
- Move more release provenance into reproducible, attestable CI steps while
  preserving Apple signing and notarization requirements.

## Distribution

- Continue publishing signed, notarized releases and the maintained Homebrew
  tap.
- Submit Zeus to the official Homebrew cask repository once it is eligible.
  Homebrew normally requires a repository to be at least 30 days old and applies
  a higher notability threshold to owner submissions. Until then, the supported
  command is `brew install --cask nnayz/zeus/zeus`.

## Not planned

- A hosted Zeus account or telemetry service.
- Treating agent processes as a security sandbox. Zeus orchestrates trusted
  developer tools; it does not contain them.
