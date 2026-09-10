import SwiftUI

struct ContentView: View {
    @ObservedObject var model: CompanionModel
    @State private var pairing = ""
    @State private var deviceName = "My iPhone"
    @State private var showingPairing = false

    var body: some View {
        NavigationStack {
            Group {
                if model.client == nil { pairingView } else { sessionView }
            }
            .navigationTitle("Zeus Companion")
            .toolbar { if model.client != nil { Button("Unpair", role: .destructive) { model.unpair() } } }
            .alert("Companion", isPresented: .constant(model.error != nil), presenting: model.error) { _ in Button("OK") { model.error = nil } } message: { Text($0) }
        }
    }

    private var pairingView: some View {
        Form {
            Section("Pair with Zeus") {
                Text("On the trusted Zeus computer, create a pairing payload and paste it here.")
                    .font(.subheadline).foregroundStyle(.secondary)
                TextEditor(text: $pairing).frame(minHeight: 120).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Device name", text: $deviceName)
                Button("Pair device") { Task { await model.pair(payloadText: pairing, deviceName: deviceName) } }
                    .disabled(pairing.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            Section { Label("HTTPS is required. The pairing token is stored in Keychain.", systemImage: "lock.shield") .font(.footnote).foregroundStyle(.secondary) }
        }
    }

    private var sessionView: some View {
        List {
            Section {
                HStack { Label(model.status, systemImage: "checkmark.circle.fill"); Spacer(); Button("Refresh") { Task { await model.refresh() } } }
            }
            Section("Sessions") {
                if model.sessions.isEmpty { ContentUnavailableView("No sessions", systemImage: "rectangle.stack") }
                ForEach(model.sessions) { session in
                    Button { Task { await model.select(session) } } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(session.title).font(.headline).foregroundStyle(.primary)
                            Text("\(session.kind) · \(session.status)\n\(session.cwd)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if let selected = model.selected { SessionDetailView(model: model, session: selected) }
        }
        .refreshable { await model.refresh() }
        .task { await model.refresh() }
    }
}

private struct SessionDetailView: View {
    @ObservedObject var model: CompanionModel
    let session: Session
    @State private var prompt = ""
    @State private var control: ControlState?

    var body: some View {
        Section("\(session.title) · \(session.status)") {
            if let screen = model.screen {
                ScrollView([.horizontal, .vertical]) { Text(screen.text.isEmpty ? "(no screen output)" : screen.text).font(.system(.footnote, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(8) }
                    .frame(minHeight: 180, maxHeight: 360).background(.black, in: RoundedRectangle(cornerRadius: 8)).foregroundStyle(.green)
            }
            if let control { Text("Control sequence \(control.commandSeq)").font(.caption).foregroundStyle(.secondary) }
            Button("Take control") { Task { await takeControl() } }
            HStack { TextField("Prompt", text: $prompt, axis: .vertical); Button("Send") { Task { await send() } }.disabled(prompt.isEmpty || control == nil) }
        }
    }
    private func takeControl() async {
        guard let client = model.client, let currentScreen = model.screen else { return }
        do { control = try await client.acquire(session.id, expected: currentScreen.control.epoch) } catch let issue { model.error = issue.localizedDescription }
    }
    private func send() async {
        guard let client = model.client, let control, !prompt.isEmpty else { return }
        do { self.control = try await client.send(session.id, expected: control.epoch, commandSeq: control.commandSeq, text: prompt); prompt = ""; await model.select(session) } catch let issue { model.error = issue.localizedDescription }
    }
}
