import Foundation

/// Free-form JSON value (flag values, experiment variants, payloads, properties).
public enum JSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null; return }
        if let b = try? c.decode(Bool.self) { self = .bool(b); return }
        if let n = try? c.decode(Double.self) { self = .number(n); return }
        if let s = try? c.decode(String.self) { self = .string(s); return }
        if let a = try? c.decode([JSONValue].self) { self = .array(a); return }
        if let o = try? c.decode([String: JSONValue].self) { self = .object(o); return }
        throw DecodingError.dataCorruptedError(in: c, debugDescription: "unsupported JSON value")
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let b): try c.encode(b)
        case .number(let n): try c.encode(n)
        case .string(let s): try c.encode(s)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }

    public var boolValue: Bool? { if case .bool(let b) = self { return b }; return nil }
    public var doubleValue: Double? { if case .number(let n) = self { return n }; return nil }
    public var intValue: Int? { doubleValue.map { Int($0) } }
    public var stringValue: String? { if case .string(let s) = self { return s }; return nil }
    public var arrayValue: [JSONValue]? { if case .array(let a) = self { return a }; return nil }
    public var objectValue: [String: JSONValue]? { if case .object(let o) = self { return o }; return nil }
    public subscript(key: String) -> JSONValue? { objectValue?[key] }
}

extension JSONValue: ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral, ExpressibleByFloatLiteral,
    ExpressibleByStringLiteral, ExpressibleByArrayLiteral, ExpressibleByDictionaryLiteral, ExpressibleByNilLiteral {
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(integerLiteral value: Int) { self = .number(Double(value)) }
    public init(floatLiteral value: Double) { self = .number(value) }
    public init(stringLiteral value: String) { self = .string(value) }
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, JSONValue)...) { self = .object(Dictionary(uniqueKeysWithValues: elements)) }
    public init(nilLiteral: ()) { self = .null }
}

public typealias PropertyMap = [String: JSONValue]

public struct Properties: Codable, Equatable, Sendable {
    public var `default`: PropertyMap
    public var computed: PropertyMap
    public var custom: PropertyMap
}

public struct Identity: Codable, Equatable, Sendable {
    public let id: String
    public let externalId: String
    public let createdAt: String
    public let updatedAt: String
    public let lastSeenAt: String

    enum CodingKeys: String, CodingKey {
        case id, externalId = "external_id", createdAt = "created_at", updatedAt = "updated_at", lastSeenAt = "last_seen_at"
    }
}

public struct Session: Codable, Equatable, Sendable {
    public let token: String
    public let refreshToken: String
    public let identity: Identity
    public let properties: Properties

    enum CodingKeys: String, CodingKey { case token, refreshToken = "refresh_token", identity, properties }
}

public struct Reason: Codable, Equatable, Sendable {
    /// disabled | default | override | experiment | rule | variant
    public let kind: String
    public let ruleId: String?
    public let variant: String?
    public let experiment: String?

    enum CodingKeys: String, CodingKey { case kind, ruleId = "rule_id", variant, experiment }
}

public struct Flag: Codable, Equatable, Sendable {
    public let key: String
    public let value: JSONValue
    /// bool | string | number | json
    public let valueType: String
    public let variant: String?
    public let reason: Reason
    public let version: Int64

    enum CodingKeys: String, CodingKey { case key, value, valueType = "value_type", variant, reason, version }
}

public struct FlagOverride: Codable, Equatable, Sendable {
    public let flagKey: String
    public let value: JSONValue
    /// identity | experiment | live_event
    public let source: String
    public let sourceKey: String?
    public let variant: String?

    enum CodingKeys: String, CodingKey { case flagKey = "flag_key", value, source, sourceKey = "source_key", variant }
}

public struct Experiment: Codable, Equatable, Sendable {
    public let key: String
    public let name: String
    public let variant: String
    public let value: JSONValue
    public let version: Int64
}

public struct Occurrence: Codable, Equatable, Sendable {
    public let start: String
    public let end: String
}

public struct LiveEvent: Codable, Equatable, Sendable {
    public let key: String
    public let name: String
    public let description: String
    public let value: JSONValue
    public let active: Bool
    public let current: Occurrence?
    public let next: Occurrence?
    public let version: Int64
}

public struct InboxMessage: Codable, Equatable, Sendable {
    public let id: String
    public let key: String?
    public let title: String
    public let body: String
    public let imageUrl: String
    public let payload: JSONValue
    public let priority: Int
    public let deliveredAt: String
    public let readAt: String?
    public let expiresAt: String?

    enum CodingKeys: String, CodingKey {
        case id, key, title, body, imageUrl = "image_url", payload, priority
        case deliveredAt = "delivered_at", readAt = "read_at", expiresAt = "expires_at"
    }
}

public struct PushToken: Codable, Equatable, Sendable {
    public let identityId: String
    /// fcm | apns
    public let platform: String
    public let token: String
    public let deviceId: String
    public let createdAt: String
    public let updatedAt: String
    public let failures: Int

    enum CodingKeys: String, CodingKey {
        case identityId = "identity_id", platform, token, deviceId = "device_id"
        case createdAt = "created_at", updatedAt = "updated_at", failures
    }
}

public struct EventInput: Codable, Equatable, Sendable {
    public var name: String
    public var value: String?
    public var metadata: [String: JSONValue]?
    /// ISO-8601; nil = server receipt time.
    public var timestamp: String?
    /// Idempotency key (UUID).
    public var id: String?

    public init(name: String, value: String? = nil, metadata: [String: JSONValue]? = nil, timestamp: String? = nil, id: String? = nil) {
        self.name = name
        self.value = value
        self.metadata = metadata
        self.timestamp = timestamp
        self.id = id
    }
}

/// Error returned by the server or raised by the client.
public struct ZanshinError: Error, Equatable, Sendable, CustomStringConvertible {
    public let status: Int
    public let code: String
    public let message: String

    public init(status: Int, code: String, message: String) {
        self.status = status
        self.code = code
        self.message = message
    }

    public var isNotFound: Bool { status == 404 }
    public var isUnauthenticated: Bool { status == 401 }
    public var description: String { "\(code) (\(status)): \(message)" }
}

/// Messages pushed on the `/v1/stream` WebSocket.
public enum StreamMessage: Equatable, Sendable {
    case hello(identityId: String)
    case flag(Flag)
    case flagDeleted(key: String)
    case flags([Flag])
    case experiments([Experiment])
    case liveEvents([LiveEvent])
    case inbox([InboxMessage])
    case pong
    case error(message: String)
    case unknown(type: String)
}

struct Envelope<T: Decodable>: Decodable {
    let value: T
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        guard let key = c.allKeys.first(where: { $0.stringValue != "next" }) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "empty envelope"))
        }
        value = try c.decode(T.self, forKey: key)
    }
}

struct AnyKey: CodingKey {
    var stringValue: String
    var intValue: Int? { nil }
    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
}
