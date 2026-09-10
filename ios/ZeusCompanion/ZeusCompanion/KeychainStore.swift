import Foundation
import Security

enum KeychainStore {
    private static let service = "com.zeus.companion"
    static func save(token: String, origin: URL, serverID: String) throws {
        let data = try JSONEncoder().encode(["token": token, "origin": origin.absoluteString, "serverID": serverID])
        let query: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: serverID]
        SecItemDelete(query as CFDictionary)
        var add = query; add[kSecValueData] = data
        guard SecItemAdd(add as CFDictionary, nil) == errSecSuccess else { throw CompanionError.http(0, "Keychain unavailable") }
    }
    static func load() -> (token: String, origin: URL, serverID: String)? {
        let query: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecReturnData: true, kSecMatchLimit: kSecMatchLimitOne]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data,
              let values = try? JSONDecoder().decode([String: String].self, from: data), let token = values["token"],
              let originString = values["origin"], let origin = URL(string: originString), let serverID = values["serverID"] else { return nil }
        return (token, origin, serverID)
    }
    static func remove(serverID: String?) { var query: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service]; if let serverID { query[kSecAttrAccount] = serverID }; SecItemDelete(query as CFDictionary) }
}
