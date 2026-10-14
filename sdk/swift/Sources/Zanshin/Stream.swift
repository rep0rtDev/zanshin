import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Live change stream for one session over the `/v1/stream` WebSocket. Keeps a local cache of
/// the latest flags, pings every 30 s, reconnects with exponential backoff and re-authenticates
/// after token refresh.
///
/// ```swift
/// let stream = ZanshinStream(client: z)
/// await stream.onFlag { flag in apply(flag) }
/// await stream.connect()
/// ```
public actor ZanshinStream {
    public struct Options: Sendable {
        /// Only receive changes for these flags (experiments / live events / inbox are always pushed).
        public var names: [String]? = nil
        public var reconnect = true
        public var pingInterval: TimeInterval = 30
        public init() {}
    }

    private let client: ZanshinClient
    private let options: Options
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?
    private var pingLoop: Task<Void, Never>?
    private var closed = true
    private var attempt = 0
    private var flagHandlers: [@Sendable (Flag) -> Void] = []
    private var flagDeletedHandlers: [@Sendable (String) -> Void] = []
    private var messageHandlers: [@Sendable (StreamMessage) -> Void] = []

    /// Latest known flags by key (updated from the stream).
    public private(set) var flags: [String: Flag] = [:]
    public var connected: Bool { task != nil && task?.state == .running }

    public init(client: ZanshinClient, options: Options = Options(), session: URLSession = .shared) {
        self.client = client
        self.options = options
        self.session = session
    }

    public func onFlag(_ h: @escaping @Sendable (Flag) -> Void) { flagHandlers.append(h) }
    public func onFlagDeleted(_ h: @escaping @Sendable (String) -> Void) { flagDeletedHandlers.append(h) }
    /// Every message (experiments, live events, inbox, errors…).
    public func onMessage(_ h: @escaping @Sendable (StreamMessage) -> Void) { messageHandlers.append(h) }

    public func connect() async {
        closed = false
        await open()
    }

    /// Stop and do not reconnect.
    public func close() {
        closed = true
        pingLoop?.cancel(); pingLoop = nil
        receiveLoop?.cancel(); receiveLoop = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
    }

    private func open() async {
        guard !closed else { return }
        guard let token = await client.token else {
            await scheduleReconnect()
            return
        }
        var s = client.url.absoluteString
        if s.hasPrefix("https") { s = "wss" + s.dropFirst(5) } else if s.hasPrefix("http") { s = "ws" + s.dropFirst(4) }
        var comps = URLComponents(string: s + "/v1/stream")!
        var items = [URLQueryItem(name: "token", value: token)]
        if let names = options.names, !names.isEmpty { items.append(URLQueryItem(name: "names", value: names.joined(separator: ","))) }
        comps.queryItems = items
        let ws = session.webSocketTask(with: comps.url!)
        task = ws
        ws.resume()
        attempt = 0
        pingLoop = Task { [weak self, interval = options.pingInterval] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
                guard !Task.isCancelled, let self else { return }
                await self.send(#"{"type":"ping"}"#)
            }
        }
        receiveLoop = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let msg = try await ws.receive()
                    guard let self else { return }
                    switch msg {
                    case .string(let text): await self.dispatch(text)
                    case .data(let data): await self.dispatch(String(decoding: data, as: UTF8.self))
                    @unknown default: break
                    }
                } catch {
                    guard let self else { return }
                    await self.handleClose()
                    return
                }
            }
        }
    }

    private func send(_ text: String) {
        guard let task else { return }
        Task { try? await task.send(.string(text)) }
    }

    private func handleClose() async {
        pingLoop?.cancel(); pingLoop = nil
        task = nil
        await scheduleReconnect()
    }

    private func scheduleReconnect() async {
        guard !closed, options.reconnect else { return }
        let delay = min(30.0, pow(2.0, Double(min(attempt, 5)))) * (0.5 + Double.random(in: 0..<1))
        attempt += 1
        try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
        guard !closed else { return }
        if await client.token == nil {
            do { _ = try await client.refresh() } catch { await scheduleReconnect(); return }
        }
        await open()
    }

    private func dispatch(_ text: String) async {
        guard let data = text.data(using: .utf8),
              let raw = try? JSONDecoder().decode([String: JSONValue].self, from: data),
              let type = raw["type"]?.stringValue else { return }
        let dec = JSONDecoder()
        func field<T: Decodable>(_ k: String, _: T.Type) -> T? {
            guard let v = raw[k], let d = try? JSONEncoder().encode(v) else { return nil }
            return try? dec.decode(T.self, from: d)
        }
        let msg: StreamMessage
        switch type {
        case "hello": msg = .hello(identityId: raw["identity_id"]?.stringValue ?? "")
        case "flag":
            guard let f = field("flag", Flag.self) else { return }
            flags[f.key] = f
            flagHandlers.forEach { $0(f) }
            msg = .flag(f)
        case "flag_deleted":
            let key = raw["key"]?.stringValue ?? ""
            flags.removeValue(forKey: key)
            flagDeletedHandlers.forEach { $0(key) }
            msg = .flagDeleted(key: key)
        case "flags":
            let fs = field("flags", [Flag].self) ?? []
            for f in fs { flags[f.key] = f }
            msg = .flags(fs)
        case "experiments": msg = .experiments(field("experiments", [Experiment].self) ?? [])
        case "live_events": msg = .liveEvents(field("live_events", [LiveEvent].self) ?? [])
        case "inbox": msg = .inbox(field("messages", [InboxMessage].self) ?? [])
        case "pong": msg = .pong
        case "error":
            let m = raw["message"]?.stringValue ?? ""
            if m.lowercased().contains("expired") { _ = try? await client.refresh() }
            msg = .error(message: m)
        default: msg = .unknown(type: type)
        }
        messageHandlers.forEach { $0(msg) }
    }
}
