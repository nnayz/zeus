import Foundation

/// A JSON-shaped value for payloads whose schema the transport layer doesn't
/// need to know (hook payloads forwarded verbatim, method params routed by name).
public enum JSONValue: Codable, Hashable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let b = try? container.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? container.decode(Double.self) {
            self = .number(n)
        } else if let s = try? container.decode(String.self) {
            self = .string(s)
        } else if let a = try? container.decode([JSONValue].self) {
            self = .array(a)
        } else if let o = try? container.decode([String: JSONValue].self) {
            self = .object(o)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Not a JSON value")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let b): try container.encode(b)
        case .number(let n): try container.encode(n)
        case .string(let s): try container.encode(s)
        case .array(let a): try container.encode(a)
        case .object(let o): try container.encode(o)
        }
    }

    // MARK: Convenience accessors

    public var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    public var objectValue: [String: JSONValue]? {
        if case .object(let o) = self { return o }
        return nil
    }

    public subscript(key: String) -> JSONValue? {
        objectValue?[key]
    }

    /// Re-encodes any Encodable into a JSONValue.
    public init<T: Encodable>(encoding value: T) throws {
        let data = try JSONEncoder.dirijor.encode(value)
        self = try JSONDecoder.dirijor.decode(JSONValue.self, from: data)
    }

    /// Decodes this JSONValue into a concrete Decodable.
    public func decoded<T: Decodable>(as type: T.Type = T.self) throws -> T {
        let data = try JSONEncoder.dirijor.encode(self)
        return try JSONDecoder.dirijor.decode(T.self, from: data)
    }
}

extension JSONEncoder {
    /// Canonical encoder for all Dirijor wire traffic and persisted state.
    public static var dirijor: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }
}

extension JSONDecoder {
    public static var dirijor: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        return decoder
    }
}
