import Foundation

/// Protocol version. Additive changes only within a major; a major bump means
/// the app must upgrade the daemon before talking to it.
public enum WireVersion {
    public static let current = 1
}

public struct ControlError: Codable, Hashable, Sendable, Error {
    public var code: String
    public var message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }

    public static func notFound(_ what: String) -> ControlError {
        ControlError(code: "not_found", message: what)
    }
    public static func badRequest(_ why: String) -> ControlError {
        ControlError(code: "bad_request", message: why)
    }
    public static func internalError(_ why: String) -> ControlError {
        ControlError(code: "internal", message: why)
    }
    public static func versionMismatch(_ why: String) -> ControlError {
        ControlError(code: "version_mismatch", message: why)
    }
    public static func unauthorized() -> ControlError {
        ControlError(code: "unauthorized", message: "invalid or missing token")
    }
}

/// One newline-delimited JSON message on the control channel.
public enum ControlMessage: Sendable {
    case request(id: UInt64, method: String, params: JSONValue?)
    case response(id: UInt64, result: Result<JSONValue, ControlError>)
    case event(name: String, seq: UInt64, params: JSONValue)
}

extension ControlMessage: Codable {
    private enum CodingKeys: String, CodingKey {
        case id, method, params, ok, err, event, seq
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if let method = try c.decodeIfPresent(String.self, forKey: .method) {
            self = .request(
                id: try c.decode(UInt64.self, forKey: .id),
                method: method,
                params: try c.decodeIfPresent(JSONValue.self, forKey: .params)
            )
        } else if let event = try c.decodeIfPresent(String.self, forKey: .event) {
            self = .event(
                name: event,
                seq: try c.decode(UInt64.self, forKey: .seq),
                params: try c.decodeIfPresent(JSONValue.self, forKey: .params) ?? .null
            )
        } else if let err = try c.decodeIfPresent(ControlError.self, forKey: .err) {
            self = .response(id: try c.decode(UInt64.self, forKey: .id), result: .failure(err))
        } else {
            self = .response(
                id: try c.decode(UInt64.self, forKey: .id),
                result: .success(try c.decodeIfPresent(JSONValue.self, forKey: .ok) ?? .null)
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .request(let id, let method, let params):
            try c.encode(id, forKey: .id)
            try c.encode(method, forKey: .method)
            try c.encodeIfPresent(params, forKey: .params)
        case .response(let id, .success(let ok)):
            try c.encode(id, forKey: .id)
            try c.encode(ok, forKey: .ok)
        case .response(let id, .failure(let err)):
            try c.encode(id, forKey: .id)
            try c.encode(err, forKey: .err)
        case .event(let name, let seq, let params):
            try c.encode(name, forKey: .event)
            try c.encode(seq, forKey: .seq)
            try c.encode(params, forKey: .params)
        }
    }
}

/// Accumulates raw bytes and yields complete newline-delimited JSON messages.
/// Not thread-safe by design — confine to one connection's reader.
public struct NDJSONBuffer: Sendable {
    private var buffer = Data()
    /// Bound on a single control line; a peer exceeding it is misbehaving.
    public static let maxLineBytes = 4 * 1024 * 1024

    public init() {}

    public mutating func append(_ data: Data) throws -> [ControlMessage] {
        buffer.append(data)
        guard buffer.count <= Self.maxLineBytes || buffer.contains(0x0A) else {
            throw ControlError.badRequest("control line exceeds \(Self.maxLineBytes) bytes")
        }
        var messages: [ControlMessage] = []
        while let newline = buffer.firstIndex(of: 0x0A) {
            let line = buffer.subdata(in: buffer.startIndex..<newline)
            buffer.removeSubrange(buffer.startIndex...newline)
            guard !line.isEmpty else { continue }
            messages.append(try JSONDecoder.dirijor.decode(ControlMessage.self, from: line))
        }
        return messages
    }

    public static func encode(_ message: ControlMessage) throws -> Data {
        var data = try JSONEncoder.dirijor.encode(message)
        data.append(0x0A)
        return data
    }
}
