import Foundation
import SwiftUI

final class CompanionClient {
    let origin: URL
    private(set) var token: String
    let serverID: String

    init(origin: URL, token: String, serverID: String) throws {
        guard origin.scheme == "https" else { throw CompanionError.insecureOrigin }
        guard origin.host != nil else { throw CompanionError.invalidPairing }
        self.origin = origin
        self.token = token
        self.serverID = serverID
    }

    static func pair(_ payload: PairPayload, deviceName: String) async throws -> (CompanionClient, PairResponse) {
        guard payload.origin.scheme == "https" else { throw CompanionError.insecureOrigin }
        guard payload.expiresAtMS > UInt64(Date().timeIntervalSince1970 * 1000) else {
            throw CompanionError.expiredPairing
        }

        let client = try CompanionClient(origin: payload.origin, token: "", serverID: payload.serverID)
        let response: PairResponse = try await client.request(
            "/v1/pair",
            method: "POST",
            body: PairRequest(
                apiMajor: 1,
                expectedServerID: payload.serverID,
                code: payload.code,
                deviceName: deviceName
            ),
            authenticated: false
        )

        guard response.serverID == payload.serverID else { throw CompanionError.serverMismatch }
        let paired = try CompanionClient(origin: payload.origin, token: response.token, serverID: response.serverID)
        let hello: Hello = try await paired.request("/v1/hello")
        guard hello.serverID == response.serverID else { throw CompanionError.serverMismatch }
        guard hello.apiMajor == 1 else { throw CompanionError.unsupportedVersion }
        return (paired, response)
    }

    func hello() async throws -> Hello {
        try await request("/v1/hello")
    }

    func sessions() async throws -> [Session] {
        let page: Page<Session> = try await request("/v1/sessions?offset=0&limit=64")
        return page.items
    }

    func session(_ id: String) async throws -> SessionDetail {
        try await request("/v1/sessions/\(id)")
    }

    func screen(_ id: String) async throws -> Screen {
        try await request("/v1/sessions/\(id)/screen")
    }

    func acquire(_ id: String, expected: ControlEpoch) async throws -> ControlState {
        try await request(
            "/v1/sessions/\(id)/control/acquire",
            method: "POST",
            body: AcquireControl(expected: expected, takeover: true)
        )
    }

    func release(_ id: String, expected: ControlEpoch) async throws -> ControlState {
        try await request(
            "/v1/sessions/\(id)/control/release",
            method: "POST",
            body: ReleaseControl(expected: expected)
        )
    }

    func send(_ id: String, expected: ControlEpoch, commandSeq: UInt64, text: String, submit: Bool = true) async throws -> ControlState {
        try await request(
            "/v1/sessions/\(id)/text",
            method: "POST",
            body: SendText(expected: expected, commandSeq: commandSeq, text: text, submit: submit)
        )
    }

    func mutate(_ id: String, mutation: Mutation) async throws -> MutationResult {
        try await request(
            "/v1/sessions/\(id)/actions",
            method: "POST",
            body: mutation
        )
    }

    private func request<T: Decodable>(
        _ path: String,
        method: String = "GET",
        body: (any Encodable)? = nil,
        authenticated: Bool = true
    ) async throws -> T {
        guard let url = URL(string: path, relativeTo: origin)?.absoluteURL else {
            throw CompanionError.invalidPairing
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = 10
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        if authenticated {
            guard !token.isEmpty else { throw CompanionError.missingToken }
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        if let body {
            request.httpBody = try JSONEncoder().encode(AnyEncodable(body))
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw CompanionError.http(0, "Invalid response")
        }

        guard (200..<300).contains(http.statusCode) else {
            let error = try? JSONDecoder().decode(APIError.self, from: data)
            throw CompanionError.http(http.statusCode, error?.code ?? "request_failed")
        }

        return try JSONDecoder().decode(T.self, from: data)
    }
}

private struct AnyEncodable: Encodable {
    let encodeValue: (Encoder) throws -> Void
    init(_ value: any Encodable) { encodeValue = value.encode }
    func encode(to encoder: Encoder) throws { try encodeValue(encoder) }
}

// MARK: - Companion Workspace Model

@MainActor
final class CompanionModel: ObservableObject {
    @Published var client: CompanionClient?
    @Published var hello: Hello?
    @Published var sessions = [Session]()
    @Published var selected: Session?
    @Published var screen: Screen?
    @Published var control: ControlState?
    @Published var blocks = [WorkspaceBlock]()
    @Published var isGenerating = false
    @Published var isRefreshing = false
    @Published var isAcquiringControl = false
    @Published var isSendingText = false
    @Published var error: String?
    @Published var status = "Not paired"
    @Published var showRawTerminal = false

    private var pollingTask: Task<Void, Never>?
    private var lastObservedScreenText = ""

    init() {
        if let saved = KeychainStore.load(),
           let client = try? CompanionClient(origin: saved.origin, token: saved.token, serverID: saved.serverID) {
            self.client = client
            self.status = "Paired"
            Task {
                await self.loadInitialData()
            }
        }
    }

    deinit {
        pollingTask?.cancel()
    }

    private func loadInitialData() async {
        guard let client else { return }
        do {
            self.hello = try await client.hello()
            await refresh()
            if let first = sessions.first {
                await select(first)
            }
        } catch {
            self.status = "Offline"
        }
    }

    func pair(payloadText: String, deviceName: String) async {
        let cleaned = payloadText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let data = cleaned.data(using: .utf8) else {
            self.error = "Invalid JSON format."
            return
        }

        do {
            isRefreshing = true
            defer { isRefreshing = false }

            let payload = try JSONDecoder().decode(PairPayload.self, from: data)
            let (client, _) = try await CompanionClient.pair(payload, deviceName: deviceName)
            self.client = client
            KeychainStore.remove(serverID: nil)
            try KeychainStore.save(token: client.token, origin: client.origin, serverID: client.serverID)
            self.hello = try? await client.hello()
            self.status = "Connected"
            Haptic.success()
            await refresh()
            if let first = sessions.first {
                await select(first)
            }
        } catch let issue {
            Haptic.error()
            self.error = issue.localizedDescription
        }
    }

    func refresh() async {
        guard let client else { return }
        isRefreshing = true
        defer { isRefreshing = false }

        do {
            sessions = try await client.sessions()
            status = "Connected"
            // If selected session was removed or updated, keep in sync
            if let current = selected, let updated = sessions.first(where: { $0.id == current.id }) {
                self.selected = updated
            }
        } catch let issue {
            self.error = issue.localizedDescription
            status = "Disconnected"
        }
    }

    func select(_ session: Session) async {
        selected = session
        blocks.removeAll()
        lastObservedScreenText = ""

        guard let client else { return }
        do {
            let fetched = try await client.screen(session.id)
            self.screen = fetched
            self.control = fetched.control
            self.syncScreenToBlocks(fetched.text)
        } catch let issue {
            self.error = issue.localizedDescription
        }

        startScreenPolling()
    }

    func startNewWorkspace() {
        stopScreenPolling()
        selected = nil
        screen = nil
        control = nil
        blocks.removeAll()
        lastObservedScreenText = ""
    }

    func refreshScreen() async {
        guard let client, let selected else { return }
        do {
            let updated = try await client.screen(selected.id)
            self.screen = updated
            self.control = updated.control
            syncScreenToBlocks(updated.text)
        } catch let issue {
            self.error = issue.localizedDescription
        }
    }

    func acquireControl() async {
        guard let client, let selected, let currentScreen = screen else { return }
        isAcquiringControl = true
        defer { isAcquiringControl = false }

        do {
            let next = try await client.acquire(selected.id, expected: currentScreen.control.epoch)
            self.control = next
            Haptic.success()
            await refreshScreen()
        } catch let issue {
            Haptic.error()
            self.error = issue.localizedDescription
        }
    }

    func releaseControl() async {
        guard let client, let selected, let currentControl = control else { return }
        do {
            let released = try await client.release(selected.id, expected: currentControl.epoch)
            self.control = released
            Haptic.light()
            await refreshScreen()
        } catch let issue {
            Haptic.error()
            self.error = issue.localizedDescription
        }
    }

    func sendPrompt(_ text: String) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        // If no session selected, pick the first active one or notify
        guard let selectedSession = selected ?? sessions.first else {
            self.error = "No active session available. Launch a session on Zeus first."
            return
        }

        if selected == nil {
            await select(selectedSession)
        }

        guard let client, let currentSession = selected else { return }

        // If not in control, attempt to acquire control first
        if control?.hasOwner == false, let currentScreen = screen {
            do {
                self.control = try await client.acquire(currentSession.id, expected: currentScreen.control.epoch)
            } catch {
                // Ignore acquire failure, backend will reject if epoch is invalid
            }
        }

        guard let currentControl = control else {
            self.error = "Cannot acquire session control lease."
            return
        }

        // Add user prompt block immediately to the document flow
        let userBlock = WorkspaceBlock(kind: .userPrompt, content: trimmed)
        blocks.append(userBlock)
        isGenerating = true
        isSendingText = true
        Haptic.light()

        do {
            let next = try await client.send(
                currentSession.id,
                expected: currentControl.epoch,
                commandSeq: currentControl.commandSeq + 1,
                text: trimmed,
                submit: true
            )
            self.control = next

            // Brief pause to allow command output to render
            try? await Task.sleep(nanoseconds: 200_000_000) // 200ms
            await refreshScreen()
            isGenerating = false
            isSendingText = false
        } catch let issue {
            isGenerating = false
            isSendingText = false
            Haptic.error()
            self.error = issue.localizedDescription
        }
    }

    private func syncScreenToBlocks(_ screenText: String) {
        let trimmed = screenText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        guard trimmed != lastObservedScreenText else { return }
        lastObservedScreenText = trimmed

        // Check if last block is already a code block / output block
        if let lastIndex = blocks.indices.last, blocks[lastIndex].kind == .codeBlock {
            blocks[lastIndex].content = trimmed
            blocks[lastIndex].isStreaming = false
        } else {
            blocks.append(WorkspaceBlock(
                kind: .codeBlock,
                content: trimmed,
                title: selected?.title ?? "Terminal Output"
            ))
        }
    }

    func startScreenPolling() {
        stopScreenPolling()
        pollingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_500_000_000) // 1.5s
                guard let self, !Task.isCancelled else { break }
                if self.selected != nil && !self.isSendingText && !self.isAcquiringControl {
                    await self.pollScreenQuietly()
                }
            }
        }
    }

    func stopScreenPolling() {
        pollingTask?.cancel()
        pollingTask = nil
    }

    private func pollScreenQuietly() async {
        guard let client, let selected else { return }
        do {
            let updated = try await client.screen(selected.id)
            self.screen = updated
            if !isSendingText && !isAcquiringControl {
                self.control = updated.control
            }
            syncScreenToBlocks(updated.text)
        } catch {
            // Transient error in background polling, ignore
        }
    }

    func unpair() {
        stopScreenPolling()
        KeychainStore.remove(serverID: client?.serverID)
        client = nil
        hello = nil
        sessions = []
        selected = nil
        screen = nil
        control = nil
        blocks.removeAll()
        status = "Not paired"
        Haptic.light()
    }
}
