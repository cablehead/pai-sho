import Foundation

/// One peer from the daemon's `List` reply.
public struct Peer: Identifiable, Hashable, Sendable {
    public var key: String
    public var name: String?
    public var online: Bool
    /// Ports this peer exposes to us
    public var ports: [UInt16]
    /// The local address its ports are bound at, absent when not projected
    public var ip: String?
    /// Where each bound port listens, as `ip:port`. Nil when the daemon is
    /// older than `bound_at`; then a port listens at `ip` and its own number.
    public var boundAt: [UInt16: String]?

    public var id: String { key }

    public init(
        key: String,
        name: String? = nil,
        online: Bool = false,
        ports: [UInt16] = [],
        ip: String? = nil,
        boundAt: [UInt16: String]? = nil
    ) {
        self.key = key
        self.name = name
        self.online = online
        self.ports = ports
        self.ip = ip
        self.boundAt = boundAt
    }

    public var title: String {
        if let name, !name.isEmpty { return name }
        return String(key.prefix(8))
    }

    /// The local `ip:port` to reach `port` at, or nil while it is not bound.
    public func address(port: UInt16) -> String? {
        if let boundAt { return boundAt[port] }
        guard let ip, !ip.isEmpty else { return nil }
        return "\(ip):\(port)"
    }

    /// The page for `port`. `byName` opens `<name>.pai-sho`, which needs the
    /// daemon's resolver (a Mac). Otherwise the page opens where the port is
    /// bound, which is what an iOS app has. A port bound at 127.0.0.1 opens as
    /// `<label>.localhost`, which reaches the same listener: WebKit sends any
    /// `.localhost` name to loopback, and treats each name as a site of its
    /// own, so peers sharing 127.0.0.1 do not share cookies. App Transport
    /// Security refuses these names, http and https alike, unless the app's
    /// Info.plist makes `localhost` and its subdomains an exception, as the
    /// iOS app's does. `label` defaults to `hostLabel`.
    public func pageURL(port: UInt16, byName: Bool, label: String? = nil) -> URL? {
        if byName, let name, !name.isEmpty, let url = URL(string: "http://\(name).pai-sho:\(port)/") {
            return url
        }
        guard let address = address(port: port) else { return nil }
        if let colon = address.lastIndex(of: ":"), address[..<colon] == "127.0.0.1" {
            let local = address[address.index(after: colon)...]
            return URL(string: "http://\(label ?? hostLabel).localhost:\(local)/")
        }
        return URL(string: "http://\(address)/")
    }

    /// This peer's name as one DNS label: lowercase letters, digits and
    /// hyphens. A name with none of those falls back to the start of the key.
    public var hostLabel: String {
        Self.label(name ?? "") ?? Self.label(String(key.prefix(8))) ?? "peer"
    }

    /// `hostLabel`, with the start of the key added when another of `peers`
    /// has the same label, so that no two peers share a site.
    public func hostLabel(among peers: [Peer]) -> String {
        let mine = hostLabel
        guard peers.contains(where: { $0.key != key && $0.hostLabel == mine }) else { return mine }
        return Self.label("\(mine)-\(key.prefix(8))") ?? mine
    }

    private static func label(_ text: String) -> String? {
        let folded = text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
        var out = ""
        for scalar in folded.unicodeScalars {
            if scalar.isASCII, CharacterSet.alphanumerics.contains(scalar) {
                out.unicodeScalars.append(scalar)
            } else if !out.isEmpty, !out.hasSuffix("-") {
                out.append("-")
            }
        }
        out = String(out.prefix(63))
        while out.hasSuffix("-") { out.removeLast() }
        return out.isEmpty ? nil : out.lowercased()
    }

    /// What to show as the address of the page for `port`: `<name>.pai-sho`
    /// and the peer's own port number, whatever local address it opened at.
    public func displayAddress(port: UInt16) -> String {
        "\(title).pai-sho:\(port)"
    }

    /// `shown`, the page loading for `port`, as the user should read it: its
    /// scheme and path at `displayAddress`.
    public func displayURL(_ shown: URL, port: UInt16) -> String {
        let scheme = shown.scheme ?? "http"
        let path = shown.path.isEmpty ? "/" : shown.path
        return "\(scheme)://\(displayAddress(port: port))\(path)"
    }

    /// Whether to accept a server's certificate without checking it, for a
    /// challenge at `host:port`. Only the address the app opened a peer's page
    /// at, a `.localhost` name, qualifies. The bytes to it travel over
    /// pai-sho's encrypted link to a known peer key, so checking that the
    /// certificate names this address adds nothing. A certificate anywhere
    /// else, a page's subresources on other hosts included, is checked as
    /// usual.
    public static func trustsCertificate(method: String, host: String, port: Int, opened: URL) -> Bool {
        guard method == NSURLAuthenticationMethodServerTrust,
              let openedHost = opened.host?.lowercased(),
              openedHost.hasSuffix(".localhost"),
              let openedPort = opened.port
        else { return false }
        return host.lowercased() == openedHost && port == openedPort
    }

    /// A port's page opens over http first: the app cannot tell from the
    /// port whether the peer serves TLS. When http fails, the page is tried
    /// once more at this address, over https.
    public static func secureRetry(for url: URL) -> URL? {
        guard url.scheme == "http", var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        parts.scheme = "https"
        return parts.url
    }
}

/// A peer's port, used as the list selection.
public struct PortID: Hashable, Sendable {
    public var peer: String
    public var port: UInt16

    public init(peer: String, port: UInt16) {
        self.peer = peer
        self.port = port
    }

    /// `<peer key>:<port>`, for keeping per-port things on disk.
    public var storageKey: String { "\(peer):\(port)" }

    public init?(storageKey: String) {
        guard let colon = storageKey.lastIndex(of: ":"),
              let port = UInt16(storageKey[storageKey.index(after: colon)...])
        else { return nil }
        let peer = String(storageKey[..<colon])
        guard !peer.isEmpty else { return nil }
        self.init(peer: peer, port: port)
    }
}

public enum DaemonError: Error, LocalizedError, Equatable {
    case notRunning
    /// The socket is there but belongs to someone else, as when the daemon
    /// runs as root and keeps its socket to itself.
    case noAccess(String)
    case badResponse
    case refused(String)

    public var errorDescription: String? {
        switch self {
        case .notRunning:
            return "pai-sho is not running."
        case .noAccess(let path):
            return "This user may not use \(path)."
        case .badResponse:
            return "pai-sho sent a response this app could not read."
        case .refused(let message):
            return message
        }
    }
}

/// Talks to a pai-sho process over its Unix socket, one JSON line each way.
/// The Mac app talks to the daemon at /tmp/pai-sho.sock. The iOS app runs the
/// process itself and talks to it at a path relative to the app's working
/// directory, because a socket path cannot be longer than 103 bytes and an
/// app's container path can be (in the simulator it is).
public struct DaemonClient: Sendable {
    public var socketPath: String

    public init(socketPath: String = "/tmp/pai-sho.sock") {
        self.socketPath = socketPath
    }

    public func list() async throws -> [Peer] {
        try Self.peers(fromListReply: try await exchange("\"List\"\n"))
    }

    public func accept(handle: String, name: String?) async throws {
        let trimmed = handle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { throw DaemonError.refused("Paste or scan an invitation.") }
        var body: [String: Any] = ["handle": trimmed]
        if let name, !name.isEmpty {
            body["name"] = name
        } else {
            body["name"] = NSNull()
        }
        let request = try JSONSerialization.data(withJSONObject: ["Accept": body])
        guard var line = String(data: request, encoding: .utf8) else { throw DaemonError.badResponse }
        line.append("\n")
        let data = try await exchange(line)
        if let text = String(data: data, encoding: .utf8), text == "\"Ok\"" { return }
        let object = try Self.jsonObject(data)
        if let message = object["Error"] as? String { throw DaemonError.refused(message) }
        if object["Ok"] != nil { return }
        throw DaemonError.badResponse
    }

    /// A new one-time invitation, valid 5 minutes. `name` is what to call the
    /// peer that takes it up.
    public func invite(name: String?) async throws -> String {
        var body: [String: Any] = ["key": NSNull(), "expose": [Int]()]
        if let name, !name.isEmpty {
            body["name"] = name
        } else {
            body["name"] = NSNull()
        }
        let request = try JSONSerialization.data(withJSONObject: ["Invite": body])
        guard var line = String(data: request, encoding: .utf8) else { throw DaemonError.badResponse }
        line.append("\n")
        return try Self.invitation(fromInviteReply: try await exchange(line))
    }

    /// Parse an `Invite` reply.
    public static func invitation(fromInviteReply data: Data) throws -> String {
        let object = try jsonObject(data)
        if let message = object["Error"] as? String { throw DaemonError.refused(message) }
        guard let invite = object["Invite"] as? String, !invite.isEmpty else { throw DaemonError.badResponse }
        return invite
    }

    /// Parse a `List` reply. Online peers first, then by title.
    public static func peers(fromListReply data: Data) throws -> [Peer] {
        let object = try jsonObject(data)
        if let message = object["Error"] as? String { throw DaemonError.refused(message) }
        guard let list = object["List"] as? [String: Any],
              let rows = list["peers"] as? [[String: Any]]
        else { throw DaemonError.badResponse }
        return rows.compactMap(peer(from:)).sorted { lhs, rhs in
            if lhs.online != rhs.online { return lhs.online && !rhs.online }
            return lhs.title.localizedCaseInsensitiveCompare(rhs.title) == .orderedAscending
        }
    }

    private static func port(_ value: Any?) -> UInt16? {
        if let number = value as? Int { return UInt16(exactly: number) }
        if let number = value as? UInt16 { return number }
        return nil
    }

    private static func peer(from row: [String: Any]) -> Peer? {
        guard let key = row["key"] as? String else { return nil }
        let ports = (row["they_expose"] as? [Any] ?? []).compactMap(port)
        var boundAt: [UInt16: String]?
        if let rows = row["bound_at"] as? [[String: Any]] {
            var map: [UInt16: String] = [:]
            for entry in rows {
                if let port = port(entry["port"]), let at = entry["at"] as? String {
                    map[port] = at
                }
            }
            boundAt = map
        }
        return Peer(
            key: key,
            name: row["name"] as? String,
            online: row["online"] as? Bool ?? false,
            ports: ports,
            ip: row["ip"] as? String,
            boundAt: boundAt
        )
    }

    private static func jsonObject(_ data: Data) throws -> [String: Any] {
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw DaemonError.badResponse
        }
        return object
    }

    private func exchange(_ line: String) async throws -> Data {
        let path = socketPath
        return try await Task.detached(priority: .userInitiated) {
            try Self.exchangeSync(line, path: path)
        }.value
    }

    private static func exchangeSync(_ line: String, path: String) throws -> Data {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonError.notRunning }
        defer { close(fd) }
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let room = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < room else { throw DaemonError.notRunning }
        withUnsafeMutablePointer(to: &address.sun_path) { pointer in
            pointer.withMemoryRebound(to: UInt8.self, capacity: room) { destination in
                bytes.withUnsafeBufferPointer { source in
                    destination.update(from: source.baseAddress!, count: bytes.count)
                    destination[bytes.count] = 0
                }
            }
        }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            throw errno == EACCES ? DaemonError.noAccess(path) : DaemonError.notRunning
        }
        try writeAll(fd, Data(line.utf8))
        return try readLine(fd)
    }

    private static func writeAll(_ fd: Int32, _ data: Data) throws {
        try data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            var sent = 0
            while sent < raw.count {
                let n = write(fd, base.advanced(by: sent), raw.count - sent)
                if n < 0 { throw DaemonError.notRunning }
                sent += n
            }
        }
    }

    private static func readLine(_ fd: Int32) throws -> Data {
        var buffer = [UInt8](repeating: 0, count: 8192)
        var data = Data()
        while !data.contains(0x0A) {
            let n = read(fd, &buffer, buffer.count)
            if n < 0 { throw DaemonError.badResponse }
            if n == 0 { break }
            data.append(buffer, count: n)
            if data.count > 2_000_000 { throw DaemonError.badResponse }
        }
        guard let end = data.firstIndex(of: 0x0A) else { throw DaemonError.badResponse }
        return data.prefix(upTo: end)
    }
}
