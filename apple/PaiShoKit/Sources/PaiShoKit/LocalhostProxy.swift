import Foundation
import Network
import OSLog
import WebKit

/// A peer's page opens at `<name>.localhost:<port>`. Before iOS 26, WebKit
/// looks that name up in DNS, finds nothing and fails the load, so the page
/// never arrives. This proxy runs in the app and is handed every request the
/// app's web views make (`WKWebsiteDataStore.proxyConfigurations`, iOS 17).
/// WebKit gives it the name as written, without looking it up, and the proxy
/// sends `*.localhost` to 127.0.0.1, where the daemon binds peer ports.
/// Any other name goes where DNS says, as it would without the proxy.
///
/// It is on only where it is needed: before iOS 26. The launch argument
/// `-localhostProxy YES` turns it on anywhere, to try it on a newer simulator.
final class LocalhostProxy: @unchecked Sendable {
    static let shared = LocalhostProxy()

    /// Whether the app's web views go through the proxy.
    static var wanted: Bool {
        if UserDefaults.standard.bool(forKey: "localhostProxy") { return true }
        #if os(iOS)
        if #available(iOS 26, *) { return false }
        return true
        #else
        return false
        #endif
    }

    /// Where a request for `host` goes: a `.localhost` name to 127.0.0.1,
    /// anything else to itself.
    static func target(_ host: String) -> String {
        let h = host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return h == "localhost" || h.hasSuffix(".localhost") ? "127.0.0.1" : host
    }

    private let queue = DispatchQueue(label: "pai-sho.localhost-proxy")
    private let log = Logger(subsystem: "stream.cross.pai-sho", category: "localhost-proxy")
    private var listener: NWListener?
    private var port: UInt16?
    private var waiting: [() -> Void] = []
    private var failed = false

    /// Calls `body` on the main queue once the proxy is ready and the web
    /// views' data store goes through it; at once when it is not wanted, or
    /// could not start, so a page still loads as it would have.
    func whenReady(_ body: @escaping @MainActor () -> Void) {
        guard Self.wanted else {
            DispatchQueue.main.async { body() }
            return
        }
        queue.async {
            if self.port != nil || self.failed {
                DispatchQueue.main.async { body() }
                return
            }
            self.waiting.append { DispatchQueue.main.async { body() } }
            if self.listener == nil { self.start() }
        }
    }

    private func start() {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        guard let listener = try? NWListener(using: params) else { return finish(nil) }
        self.listener = listener
        listener.newConnectionHandler = { [weak self] c in self?.accept(c) }
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready: self.finish(listener.port?.rawValue)
            case .failed: self.finish(nil)
            default: break
            }
        }
        listener.start(queue: queue)
    }

    private func finish(_ port: UInt16?) {
        guard self.port == nil, !failed else { return }
        if let port, let nwPort = NWEndpoint.Port(rawValue: port) {
            self.port = port
            let proxy = ProxyConfiguration(httpCONNECTProxy: .hostPort(host: "127.0.0.1", port: nwPort))
            DispatchQueue.main.sync {
                WKWebsiteDataStore.default().proxyConfigurations = [proxy]
            }
        } else {
            failed = true
        }
        let ready = waiting
        waiting = []
        ready.forEach { $0() }
    }

    private func accept(_ client: NWConnection) {
        client.start(queue: queue)
        readHead(client, Data())
    }

    /// Reads the request's head: `CONNECT host:port` for a tunnel, which is
    /// what WebKit sends, or `GET http://host:port/path` for plain http.
    private func readHead(_ client: NWConnection, _ buffer: Data) {
        client.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, done, _ in
            guard let self else { return }
            var buffer = buffer
            if let data { buffer.append(data) }
            guard let end = buffer.range(of: Data("\r\n\r\n".utf8)) else {
                if done || buffer.count > 65536 { client.cancel() } else { self.readHead(client, buffer) }
                return
            }
            let head = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
            let rest = Data(buffer[end.upperBound...])
            let line = head.components(separatedBy: "\r\n").first ?? ""
            let parts = line.split(separator: " ")
            guard parts.count >= 3 else { return client.cancel() }
            if parts[0] == "CONNECT" {
                guard let (host, port) = Self.hostPort(String(parts[1]), defaultPort: 443) else { return client.cancel() }
                let ok = Data("HTTP/1.1 200 Connection Established\r\n\r\n".utf8)
                self.tunnel(client, host: host, port: port, reply: ok, send: rest)
            } else if let url = URL(string: String(parts[1])), let host = url.host {
                var path = url.path.isEmpty ? "/" : url.path
                if let q = url.query { path += "?" + q }
                let first = "\(parts[0]) \(path) \(parts[2])"
                let rewritten = head.replacingOccurrences(of: line, with: first) + "\r\n\r\n"
                self.tunnel(client, host: host, port: UInt16(url.port ?? 80), reply: nil, send: Data(rewritten.utf8) + rest)
            } else {
                client.cancel()
            }
        }
    }

    static func hostPort(_ s: String, defaultPort: UInt16) -> (String, UInt16)? {
        if s.hasPrefix("[") {
            guard let close = s.firstIndex(of: "]") else { return nil }
            let host = String(s[s.index(after: s.startIndex)..<close])
            let after = s[s.index(after: close)...]
            let port = after.hasPrefix(":") ? UInt16(after.dropFirst()) : defaultPort
            return port.map { (host, $0) }
        }
        let bits = s.split(separator: ":", omittingEmptySubsequences: false)
        guard let host = bits.first, !host.isEmpty else { return nil }
        let port = bits.count > 1 ? UInt16(bits[1]) : defaultPort
        return port.map { (String(host), $0) }
    }

    private func tunnel(_ client: NWConnection, host: String, port: UInt16, reply: Data?, send: Data) {
        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return client.cancel() }
        log.debug("\(host, privacy: .public):\(port) to \(Self.target(host), privacy: .public)")
        let upstream = NWConnection(host: .init(Self.target(host)), port: nwPort, using: .tcp)
        upstream.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                if let reply { client.send(content: reply, completion: .contentProcessed { _ in }) }
                if !send.isEmpty { upstream.send(content: send, completion: .contentProcessed { _ in }) }
                self.pipe(from: client, to: upstream)
                self.pipe(from: upstream, to: client)
            case .failed, .waiting:
                upstream.cancel()
                let bad = Data("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".utf8)
                client.send(content: bad, completion: .contentProcessed { _ in client.cancel() })
            default: break
            }
        }
        upstream.start(queue: queue)
    }

    private func pipe(from: NWConnection, to: NWConnection) {
        from.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] data, _, done, error in
            if let data, !data.isEmpty {
                to.send(content: data, completion: .contentProcessed { _ in
                    if done || error != nil {
                        to.cancel()
                        from.cancel()
                    } else {
                        self?.pipe(from: from, to: to)
                    }
                })
            } else if done || error != nil {
                to.cancel()
                from.cancel()
            } else {
                self?.pipe(from: from, to: to)
            }
        }
    }
}
