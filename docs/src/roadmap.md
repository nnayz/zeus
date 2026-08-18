# Roadmap

Direction only — not a release calendar.

## Now

- Session persistence and daemon upgrades that stay boring and recoverable
- Broader first-class agent manifests and status detection
- Stronger release supply-chain checks (signing, notarization, update feed)

## Next

- Broader first-class agent manifests and cross-platform Engine coverage
- Clearer remote-host and remote-node setup and diagnostics
- Deeper end-to-end coverage for updates and session recovery

## Distribution

- Signed, notarized GitHub Releases
- Homebrew cask (monorepo tap): `brew tap nnayz/zeus https://github.com/nnayz/zeus.git && brew install --cask nnayz/zeus/zeus`

## Not planned

- A hosted Zeus account or telemetry service
- Treating agent processes as a security sandbox
