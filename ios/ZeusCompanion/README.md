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
