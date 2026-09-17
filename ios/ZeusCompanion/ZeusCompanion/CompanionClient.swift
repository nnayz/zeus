import Foundation

final class CompanionClient {
    let origin: URL; private(set) var token: String; let serverID: String
    init(origin: URL, token: String, serverID: String) throws {
        guard origin.scheme == "https" else { throw CompanionError.insecureOrigin }
        guard origin.host != nil else { throw CompanionError.invalidPairing }
        self.origin = origin; self.token = token; self.serverID = serverID
    }
    static func pair(_ payload: PairPayload, deviceName: String) async throws -> (CompanionClient, PairResponse) {
        guard payload.origin.scheme == "https" else { throw CompanionError.insecureOrigin }
        guard payload.expiresAtMS > UInt64(Date().timeIntervalSince1970 * 1000) else { throw CompanionError.expiredPairing }
        let client = try CompanionClient(origin: payload.origin, token: "", serverID: payload.serverID)
        let response: PairResponse = try await client.request("/v1/pair", method: "POST", body: PairRequest(apiMajor: 1, expectedServerID: payload.serverID, code: payload.code, deviceName: deviceName), authenticated: false)
        guard response.serverID == payload.serverID else { throw CompanionError.serverMismatch }
        let paired = try CompanionClient(origin: payload.origin, token: response.token, serverID: response.serverID)
        let hello: Hello = try await paired.request("/v1/hello")
        guard hello.serverID == response.serverID else { throw CompanionError.serverMismatch }
        guard hello.apiMajor == 1 else { throw CompanionError.unsupportedVersion }
        return (paired, response)
    }
    func hello() async throws -> Hello { try await request("/v1/hello") }
    func sessions() async throws -> [Session] { let page: Page<Session> = try await request("/v1/sessions?offset=0&limit=64"); return page.items }
    func session(_ id: String) async throws -> SessionDetail { try await request("/v1/sessions/\(id)") }
    func screen(_ id: String) async throws -> Screen { try await request("/v1/sessions/\(id)/screen") }
    func acquire(_ id: String, expected: ControlEpoch) async throws -> ControlState { try await request("/v1/sessions/\(id)/control/acquire", method: "POST", body: AcquireControl(expected: expected, takeover: true)) }
    func send(_ id: String, expected: ControlEpoch, commandSeq: UInt64, text: String) async throws -> ControlState { try await request("/v1/sessions/\(id)/text", method: "POST", body: SendText(expected: expected, commandSeq: commandSeq, text: text, submit: true)) }
    private func request<T: Decodable>(_ path: String, method: String = "GET", body: (any Encodable)? = nil, authenticated: Bool = true) async throws -> T {
        guard let url = URL(string: path, relativeTo: origin)?.absoluteURL else { throw CompanionError.invalidPairing }
        var request = URLRequest(url: url); request.httpMethod = method; request.timeoutInterval = 10
        request.setValue("application/json", forHTTPHeaderField: "Accept"); if authenticated { guard !token.isEmpty else { throw CompanionError.missingToken }; request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body { request.httpBody = try JSONEncoder().encode(AnyEncodable(body)); request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        let (data, response) = try await URLSession.shared.data(for: request); guard let http = response as? HTTPURLResponse else { throw CompanionError.http(0, "Invalid response") }
        guard (200..<300).contains(http.statusCode) else { let error = try? JSONDecoder().decode(APIError.self, from: data); throw CompanionError.http(http.statusCode, error?.code ?? "request_failed") }
        return try JSONDecoder().decode(T.self, from: data)
    }
}

private struct AnyEncodable: Encodable { let encodeValue: (Encoder) throws -> Void; init(_ value: any Encodable) { encodeValue = value.encode }; func encode(to encoder: Encoder) throws { try encodeValue(encoder) } }

@MainActor final class CompanionModel: ObservableObject {
    @Published var client: CompanionClient?; @Published var sessions = [Session](); @Published var selected: Session?; @Published var screen: Screen?; @Published var error: String?; @Published var status = "Not paired"
    init() { if let saved = KeychainStore.load(), let client = try? CompanionClient(origin: saved.origin, token: saved.token, serverID: saved.serverID) { self.client = client; status = "Paired" } }
    func pair(payloadText: String, deviceName: String) async { do { let payload = try JSONDecoder().decode(PairPayload.self, from: Data(payloadText.utf8)); let (client, _) = try await CompanionClient.pair(payload, deviceName: deviceName); self.client = client; KeychainStore.remove(serverID: nil); try KeychainStore.save(token: client.token, origin: client.origin, serverID: client.serverID); status = "Paired"; await refresh() } catch let issue { self.error = issue.localizedDescription } }
    func refresh() async { guard let client else { return }; do { sessions = try await client.sessions(); status = "Updated" } catch let issue { self.error = issue.localizedDescription } }
    func select(_ session: Session) async { selected = session; guard let client else { return }; do { screen = try await client.screen(session.id) } catch let issue { self.error = issue.localizedDescription } }
    func unpair() { KeychainStore.remove(serverID: client?.serverID); client = nil; sessions = []; selected = nil; screen = nil; status = "Not paired" }
}
