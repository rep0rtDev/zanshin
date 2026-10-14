import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Zanshin client for a single identity (one player / device).
///
/// ```swift
/// let z = ZanshinClient(url: URL(string: "https://liveops.example.com")!, apiKey: "zk_c_...")
/// let session = try await z.authenticate(id: deviceId, default: ["platform": "ios"])
/// let shop = try await z.flag("shop_v2").value.boolValue ?? false
/// ```
public actor ZanshinClient {
    public struct Tokens: Equatable, Sendable {
        public let token: String
        public let refreshToken: String
    }

    public nonisolated let url: URL
    private let apiKey: String
    private let session: URLSession
    private let leeway: TimeInterval
    private let onSession: (@Sendable (Tokens?) -> Void)?
    private var tokens: Tokens?
    private var expiresAt: TimeInterval = 0
    private var refreshTask: Task<Session, Error>?

    /// - Parameters:
    ///   - url: REST base URL (no trailing slash needed).
    ///   - apiKey: client API key.
    ///   - refreshLeeway: refresh the access token this many seconds before it expires.
    ///   - onSession: called whenever tokens change (persist them to resume the session later).
    public init(url: URL, apiKey: String, session: URLSession = .shared, refreshLeeway: TimeInterval = 60,
                onSession: (@Sendable (Tokens?) -> Void)? = nil) {
        var s = url.absoluteString
        while s.hasSuffix("/") { s.removeLast() }
        self.url = URL(string: s) ?? url
        self.apiKey = apiKey
        self.session = session
        self.leeway = refreshLeeway
        self.onSession = onSession
    }

    /// Current access token, or nil when not authenticated.
    public var token: String? { tokens?.token }
    public var isAuthenticated: Bool { tokens != nil }

    // MARK: - session

    /// Resume a persisted session; refreshes immediately if the access token is stale.
    public func restore(token: String, refreshToken: String) async throws {
        setTokens(Tokens(token: token, refreshToken: refreshToken), notify: false)
        if expiringSoon() { _ = try await refresh() }
    }

    /// Authenticate (create or resume) the identity `id` and open a session.
    public func authenticate(id: String, default defaults: PropertyMap = [:], custom: PropertyMap = [:]) async throws -> Session {
        let s: Session = try await request("POST", "/v1/authenticate", auth: .key,
                                           body: ["id": .string(id), "default": .object(defaults), "custom": .object(custom)])
        setTokens(Tokens(token: s.token, refreshToken: s.refreshToken))
        return s
    }

    /// Rotate tokens. Concurrent calls share one request.
    public func refresh() async throws -> Session {
        if let t = refreshTask { return try await t.value }
        guard let rt = tokens?.refreshToken else { throw ZanshinError(status: 401, code: "unauthenticated", message: "no refresh token") }
        let task = Task<Session, Error> {
            do {
                let s: Session = try await request("POST", "/v1/authenticate/refresh", auth: .key, body: ["refresh_token": .string(rt)])
                setTokens(Tokens(token: s.token, refreshToken: s.refreshToken))
                return s
            } catch let e as ZanshinError where e.status == 401 {
                clearTokens()
                throw e
            }
        }
        refreshTask = task
        defer { refreshTask = nil }
        return try await task.value
    }

    /// Revoke the session server-side and forget tokens.
    public func logout() async throws {
        guard let t = tokens else { return }
        clearTokens()
        let _: JSONValue = try await request("POST", "/v1/authenticate/logout", auth: .key,
                                             body: ["token": .string(t.token), "refresh_token": .string(t.refreshToken)])
    }

    // MARK: - identity & properties

    public func identity() async throws -> Identity { try await request("GET", "/v1/identity") }

    /// Delete the identity and everything attached to it.
    public func deleteIdentity() async throws {
        let _: JSONValue = try await request("DELETE", "/v1/identity")
        clearTokens()
    }

    public func properties() async throws -> Properties { try await request("GET", "/v1/properties") }

    /// Merge properties; a `.null` value removes the key.
    public func updateProperties(default defaults: PropertyMap = [:], custom: PropertyMap = [:]) async throws -> Properties {
        try await request("PUT", "/v1/properties", body: ["default": .object(defaults), "custom": .object(custom)])
    }

    /// Link the current (typically anonymous) identity to `id`; the client switches to the new session.
    public func identify(_ id: String) async throws -> Session {
        let s: Session = try await request("POST", "/v1/identify", body: ["id": .string(id)])
        setTokens(Tokens(token: s.token, refreshToken: s.refreshToken))
        return s
    }

    // MARK: - flags

    public func flags(names: [String]? = nil) async throws -> [Flag] {
        let r: Envelope<[Flag]> = try await request("GET", "/v1/flags", query: ["names": names?.joined(separator: ",")])
        return r.value
    }

    public func flag(_ key: String) async throws -> Flag { try await request("GET", "/v1/flags/" + encode(key)) }

    /// Flag value with a fallback when the flag does not exist.
    public func flagValue(_ key: String, fallback: JSONValue) async throws -> JSONValue {
        do { return try await flag(key).value } catch let e as ZanshinError where e.isNotFound { return fallback }
    }

    /// Every identity / experiment / live-event override applied to the caller right now.
    public func flagOverrides() async throws -> [FlagOverride] {
        let r: Envelope<[FlagOverride]> = try await request("GET", "/v1/flags/overrides")
        return r.value
    }

    public func audiences() async throws -> [String] {
        let r: Envelope<[String]> = try await request("GET", "/v1/audiences")
        return r.value
    }

    // MARK: - experiments

    public func experiments(names: [String]? = nil) async throws -> [Experiment] {
        let r: Envelope<[Experiment]> = try await request("GET", "/v1/experiments", query: ["names": names?.joined(separator: ",")])
        return r.value
    }

    /// Record that the identity actually saw its variant.
    public func expose(_ experiment: String) async throws {
        let _: JSONValue = try await request("POST", "/v1/experiments/" + encode(experiment) + "/expose")
    }

    // MARK: - live events

    public func liveEvents(names: [String]? = nil, upcoming: Bool = false) async throws -> [LiveEvent] {
        let r: Envelope<[LiveEvent]> = try await request("GET", "/v1/live-events",
                                                         query: ["names": names?.joined(separator: ","), "upcoming": upcoming ? "true" : nil])
        return r.value
    }

    /// Explicitly join a live event with `join_mode: explicit`; returns the event as now seen.
    public func joinLiveEvent(_ key: String) async throws -> LiveEvent {
        try await request("POST", "/v1/live-events/" + encode(key) + "/join")
    }

    /// Leave a live event; false when the identity was not a participant.
    public func leaveLiveEvent(_ key: String) async throws -> Bool {
        let r: JSONValue = try await request("POST", "/v1/live-events/" + encode(key) + "/leave")
        return r["left"]?.boolValue ?? false
    }

    // MARK: - inbox

    public func inbox(limit: Int? = nil) async throws -> [InboxMessage] {
        let r: Envelope<[InboxMessage]> = try await request("GET", "/v1/messages", query: ["limit": limit.map(String.init)])
        return r.value
    }

    public func markRead(_ id: String) async throws -> InboxMessage {
        try await request("PUT", "/v1/messages/" + encode(id), body: ["read": .bool(true)])
    }

    /// Consume (remove) a message; it is never redelivered.
    public func consume(_ id: String) async throws {
        let _: JSONValue = try await request("DELETE", "/v1/messages/" + encode(id))
    }

    /// Remove a message from the inbox entirely.
    public func deleteInboxMessage(_ id: String) async throws {
        let _: JSONValue = try await request("DELETE", "/v1/inbox/" + encode(id))
    }

    // MARK: - push tokens

    /// Register this device for push notifications (`fcm` registration token or `apns` hex token).
    public func registerPushToken(platform: String, token: String, deviceId: String = "") async throws -> PushToken {
        try await request("PUT", "/v1/push-tokens", body: ["platform": .string(platform), "token": .string(token), "device_id": .string(deviceId)])
    }

    public func pushTokens() async throws -> [PushToken] {
        let r: Envelope<[PushToken]> = try await request("GET", "/v1/push-tokens")
        return r.value
    }

    public func unregisterPushToken(_ token: String) async throws {
        let _: JSONValue = try await request("DELETE", "/v1/push-tokens/" + encode(token))
    }

    // MARK: - analytics

    /// Report events; returns how many were accepted.
    @discardableResult
    public func track(_ events: [EventInput]) async throws -> Int {
        if events.isEmpty { return 0 }
        let data = try JSONEncoder().encode(["events": events])
        let r: JSONValue = try await request("POST", "/v1/events", rawBody: data)
        return r["accepted"]?.intValue ?? 0
    }

    /// Convenience: one event.
    @discardableResult
    public func event(_ name: String, value: String? = nil, metadata: [String: JSONValue]? = nil) async throws -> Int {
        try await track([EventInput(name: name, value: value, metadata: metadata)])
    }

    // MARK: - transport

    enum Auth { case key, session, none }

    private func setTokens(_ t: Tokens, notify: Bool = true) {
        tokens = t
        expiresAt = ZanshinClient.decodeExp(t.token)
        if notify { onSession?(t) }
    }

    private func clearTokens() {
        tokens = nil
        expiresAt = 0
        onSession?(nil)
    }

    private func expiringSoon() -> Bool {
        guard tokens != nil else { return true }
        if expiresAt == 0 { return false }
        return Date().timeIntervalSince1970 >= expiresAt - leeway
    }

    nonisolated func encode(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? s
    }

    func request<T: Decodable>(_ method: String, _ path: String, auth: Auth = .session,
                               query: [String: String?] = [:], body: JSONValue? = nil, rawBody: Data? = nil) async throws -> T {
        var comps = URLComponents(string: url.absoluteString + path)!
        let items = query.compactMap { k, v -> URLQueryItem? in
            guard let v, !v.isEmpty else { return nil }
            return URLQueryItem(name: k, value: v)
        }
        if !items.isEmpty { comps.queryItems = items }
        var req = URLRequest(url: comps.url!)
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "accept")
        switch auth {
        case .key:
            req.setValue(apiKey, forHTTPHeaderField: "x-api-key")
        case .session:
            guard tokens != nil else { throw ZanshinError(status: 401, code: "unauthenticated", message: "not authenticated") }
            if expiringSoon(), tokens?.refreshToken.isEmpty == false { _ = try await refresh() }
            req.setValue("Bearer " + (tokens?.token ?? ""), forHTTPHeaderField: "authorization")
        case .none:
            break
        }
        if let rawBody {
            req.setValue("application/json", forHTTPHeaderField: "content-type")
            req.httpBody = rawBody
        } else if let body {
            req.setValue("application/json", forHTTPHeaderField: "content-type")
            req.httpBody = try JSONEncoder().encode(body)
        }
        let (data, resp) = try await session.data(for: req)
        let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            var code = "error"
            var message = String(data: data, encoding: .utf8) ?? ""
            if let e = try? JSONDecoder().decode([String: JSONValue].self, from: data) {
                if let c = e["code"]?.stringValue { code = c }
                if let m = e["message"]?.stringValue { message = m }
            }
            if status == 401, case .session = auth { clearTokens() }
            throw ZanshinError(status: status, code: code, message: message)
        }
        if data.isEmpty { return try JSONDecoder().decode(T.self, from: Data("{}".utf8)) }
        return try JSONDecoder().decode(T.self, from: data)
    }

    nonisolated static func decodeExp(_ jwt: String) -> TimeInterval {
        let parts = jwt.split(separator: ".")
        guard parts.count >= 2 else { return 0 }
        var b64 = String(parts[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while b64.count % 4 != 0 { b64 += "=" }
        guard let data = Data(base64Encoded: b64),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let exp = obj["exp"] as? Double else { return 0 }
        return exp
    }
}

extension URLSession {
    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        #if canImport(FoundationNetworking)
        return try await withCheckedThrowingContinuation { cont in
            dataTask(with: request) { data, resp, err in
                if let err {
                    cont.resume(throwing: err)
                } else if let resp {
                    cont.resume(returning: (data ?? Data(), resp))
                } else {
                    cont.resume(throwing: ZanshinError(status: 0, code: "transport", message: "no response"))
                }
            }.resume()
        }
        #else
        if #available(iOS 15, macOS 12, tvOS 15, watchOS 8, *) {
            return try await self.data(for: request, delegate: nil)
        }
        return try await withCheckedThrowingContinuation { cont in
            dataTask(with: request) { data, resp, err in
                if let err {
                    cont.resume(throwing: err)
                } else if let resp {
                    cont.resume(returning: (data ?? Data(), resp))
                } else {
                    cont.resume(throwing: ZanshinError(status: 0, code: "transport", message: "no response"))
                }
            }.resume()
        }
        #endif
    }
}
