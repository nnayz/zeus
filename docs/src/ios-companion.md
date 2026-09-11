# iOS Companion

The iOS Companion lets you check Zeus sessions and interact with an active
session from an iPhone or iPad. It is a native SwiftUI app in
[`ios/ZeusCompanion/`](../../ios/ZeusCompanion/), inside this repository.

The app is a client. The Rust Companion gateway runs beside Zeus and remains
the only network entry point. The phone never connects directly to the Zeus
Engine, a PTY, or SSH.

## Build and run

You need:

- macOS with Xcode 26 or newer;
- an iPhone or iPad running iOS 17 or newer, or an iOS simulator;
- a running Zeus Companion gateway reachable over the private network;
- an HTTPS origin for that gateway.

Open `ios/ZeusCompanion/ZeusCompanion.xcodeproj` in Xcode, select the
**ZeusCompanion** scheme, choose a device, and press **Run**. Signing is needed
when installing on a physical device. The project uses the bundle identifier
`com.zeus.companion`; change it to a team-owned identifier in Xcode if needed.

The native client requires HTTPS. Do not work around this by allowing arbitrary
HTTP or by disabling certificate validation. The loopback HTTP fixture used by
the web prototype is not supported by the iOS app.

## Pair the device

1. Start the Companion gateway on the trusted Mac and create a short-lived
   pairing payload.
2. Copy the complete JSON payload to the phone. It has this shape:

   ```json
   {
     "origin": "https://zeus.example",
     "server_id": "EXPECTED_SERVER_ID",
     "code": "ONE_TIME_CODE",
     "expires_at_ms": 1790000000000
   }
   ```

3. Launch Zeus Companion and paste the payload into **Pair with Zeus**.
4. Enter a name such as `Nasrul’s iPhone`, then tap **Pair device**.
5. Confirm that the server identity shown by the trusted computer matches the
   payload before using the device.

The pairing code expires quickly and is intended for one device. After pairing,
the app stores the bearer token in Keychain. It does not put the token in a URL,
UserDefaults, browser storage, or a WebSocket query string.

## Browse and control a session

After pairing, the app shows the sessions known to Zeus. Pull down to refresh,
then tap a session to load its current terminal screen.

Tap **Take control** before sending a prompt. Zeus grants a controller lease
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
send arbitrary keyboard or mouse events, or act as a replacement for the Zeus
desktop app.

Refresh is currently user initiated. A future client can add the Companion
WebSocket event stream for faster updates, but it must still refetch bounded
state after reconnects and missed events. The phone must be able to reach the
configured HTTPS gateway; background operation and push notifications are not
part of this target.
