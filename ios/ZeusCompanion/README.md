# Zeus Companion for iOS

Native SwiftUI client for the Zeus Companion HTTPS API. The iOS app talks to
the private-network gateway; it never connects to the Engine or to SSH
directly.

Open `ZeusCompanion.xcodeproj` in Xcode 26 or newer and run the `ZeusCompanion`
scheme on an iOS 17+ device or simulator. Enter the JSON pairing payload
created on the trusted Zeus computer. The app stores the resulting bearer token
in Keychain and validates the server ID and API major version before use.

The gateway must be reachable over HTTPS from the device. HTTP is intentionally
not supported by the native client.

## Local Development and Testing

To run a self-contained local testing gateway on your Mac:

```sh
./scripts/companion-dev.sh
```

This script:
- Verifies local TLS certificates (using `mkcert` or an existing certificate in `~/.zeus-companion-dev/certs`).
- Starts an isolated test Engine running an interactive shell (`/bin/zsh -l`).
- Starts an HTTPS reverse proxy on port 8443.
- Copies the fresh pairing JSON payload directly to your macOS clipboard.

Run the app in an iOS Simulator, paste the payload into **Pair with Zeus**, and tap **Pair device**.
