# iOS Companion

The iOS Companion lets you check Taskmaster sessions and interact with an active
session from an iPhone or iPad. It is a native SwiftUI app in
[`ios/TaskmasterCompanion/`](https://github.com/nnayz/taskmaster/tree/main/ios/TaskmasterCompanion),
inside this repository.

The app is a client. The Rust Companion gateway runs beside Taskmaster and remains
the only network entry point. The phone never connects directly to the Taskmaster
Engine, a PTY, or SSH.

## Build and run

You need:

- macOS with Xcode 26 or newer;
- an iPhone or iPad running iOS 17 or newer, or an iOS simulator;
- a running Taskmaster Companion gateway reachable over the private network;
- an HTTPS origin for that gateway.

Open `ios/TaskmasterCompanion/TaskmasterCompanion.xcodeproj` in Xcode, select the
**TaskmasterCompanion** scheme, choose a device, and press **Run**. Signing is needed
when installing on a physical device. The project uses the bundle identifier
`com.taskmaster.companion`; change it to a team-owned identifier in Xcode if needed.

The native client requires HTTPS. Do not work around this by allowing arbitrary
HTTP or by disabling certificate validation. The loopback HTTP fixture used by
the web prototype is not supported by the iOS app.

## Pair the device

1. Start the Companion gateway on the trusted Mac and create a short-lived
   pairing payload.
2. Copy the complete JSON payload to the phone. It has this shape:

   ```json
   {
     "origin": "https://taskmaster.example",
     "server_id": "EXPECTED_SERVER_ID",
     "code": "ONE_TIME_CODE",
     "expires_at_ms": 1790000000000
   }
   ```

3. Launch Taskmaster Companion and paste the payload into **Pair with Taskmaster**.
4. Enter a name such as `Nasrul’s iPhone`, then tap **Pair device**.
5. Confirm that the server identity shown by the trusted computer matches the
   payload before using the device.

The pairing code expires quickly and is intended for one device. After pairing,
the app stores the bearer token in Keychain. It does not put the token in a URL,
UserDefaults, browser storage, or a WebSocket query string.

## Browse and control a session

After pairing, the app shows the sessions known to Taskmaster. Pull down to refresh,
then tap a session to load its current terminal screen.

Tap **Take control** before sending a prompt. Taskmaster grants a controller lease
and returns a control sequence. Enter a prompt and tap **Send**. The gateway
checks the session identity, control epoch, and command sequence before the
Engine delivers the text to the session.

If the lease is stale, the session changed, or another controller took over,
the request fails and the app reports the error. Refresh the session screen and
explicitly take control again.

Use **Unpair** from the navigation bar when the device should no longer have
access. This removes the local Keychain token and asks the gateway to revoke the
device.

## Current boundaries

The app currently supports pairing, session browsing, screen viewing, control
acquisition, and prompt submission. It does not run agents locally, open SSH,
send arbitrary keyboard or mouse events, or act as a replacement for the Taskmaster
desktop app.

Refresh is currently user initiated. A future client can add the Companion
WebSocket event stream for faster updates, but it must still refetch bounded
state after reconnects and missed events. The phone must be able to reach the
configured HTTPS gateway; background operation and push notifications are not
part of this target.

## For contributors

The sections above are how you build and use the shipped client. The API,
trust model, gateway, and client boundaries are in the
[developer guide on GitHub](https://github.com/nnayz/taskmaster/blob/main/docs/dev/src/companion-architecture.md). Do not treat this page as
the contract.
