import Foundation
import Observation

/// The peers a pai-sho process knows, refreshed from its `List` every two
/// seconds while a screen watches it.
@MainActor
@Observable
public final class PeerStore {
    public let client: DaemonClient
    /// Open pages at `<name>.pai-sho` (a Mac with the daemon's resolver) rather
    /// than at the address each port is bound at (iOS).
    public let opensByName: Bool

    public private(set) var peers: [Peer] = []
    /// Why the last refresh failed, kept only while there are no peers to show
    public private(set) var problem: String?
    public private(set) var loadedOnce = false
    /// The last refresh reached pai-sho. Until it does, `peers` are the ones
    /// it listed last time, kept so the list is there while it starts.
    public private(set) var reachable = false
    /// Whether this app has its own pai-sho running (iOS), set by the app.
    /// Nil where pai-sho is a service of its own (the Mac).
    public var processRunning: Bool?
    /// Each page's title, as it was when last opened. Kept across launches.
    public private(set) var pageTitles: [PortID: String] = [:]
    private let defaults: UserDefaults

    public init(client: DaemonClient, opensByName: Bool, defaults: UserDefaults = .standard) {
        self.client = client
        self.opensByName = opensByName
        self.defaults = defaults
        let saved = defaults.dictionary(forKey: Self.titlesKey) as? [String: String] ?? [:]
        for (key, title) in saved {
            if let id = PortID(storageKey: key) { pageTitles[id] = title }
        }
        peers = Self.known(in: defaults)
    }

    static let knownKey = "knownPeers"

    /// The peers pai-sho listed last, as they were: who they are and what
    /// they expose, not whether they are online.
    static func known(in defaults: UserDefaults) -> [Peer] {
        let rows = defaults.array(forKey: knownKey) as? [[String: Any]] ?? []
        return rows.compactMap { row in
            guard let key = row["key"] as? String else { return nil }
            let ports = (row["ports"] as? [Int] ?? []).compactMap { UInt16(exactly: $0) }
            return Peer(key: key, name: row["name"] as? String, ports: ports)
        }
    }

    private func rememberKnown(_ peers: [Peer]) {
        let rows: [[String: Any]] = peers.map { peer in
            var row: [String: Any] = ["key": peer.key, "ports": peer.ports.map(Int.init)]
            if let name = peer.name { row["name"] = name }
            return row
        }
        let before = Self.known(in: defaults).map { [$0.key, $0.name ?? "", $0.ports.description] }
        let after = peers.map { [$0.key, $0.name ?? "", $0.ports.description] }
        if before != after { defaults.set(rows, forKey: Self.knownKey) }
    }

    /// What to say beside a peer's name.
    public func status(of peer: Peer) -> PeerStatus {
        if !reachable { return processRunning == false ? .stopped : .starting }
        if peer.online { return .online }
        if peer.asleep { return .asleep }
        if peer.dialing { return .connecting }
        return .offline
    }

    static let titlesKey = "pageTitles"

    /// Remember a page's title, to show in its port row next time.
    public func remember(title: String, for id: PortID) {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, pageTitles[id] != title else { return }
        pageTitles[id] = title
        var saved = defaults.dictionary(forKey: Self.titlesKey) as? [String: String] ?? [:]
        saved[id.storageKey] = title
        defaults.set(saved, forKey: Self.titlesKey)
    }

    /// Refresh until the calling task is cancelled.
    public func watch() async {
        while !Task.isCancelled {
            await refresh()
            try? await Task.sleep(for: .seconds(2))
        }
    }

    public func refresh() async {
        do {
            let next = try await client.list()
            loadedOnce = true
            reachable = true
            problem = nil
            if next != peers { peers = next }
            rememberKnown(next)
        } catch {
            loadedOnce = true
            reachable = false
            problem = error.localizedDescription
            // Keep showing who they are; nothing is online or bound now.
            let known = peers.map { Peer(key: $0.key, name: $0.name, ports: $0.ports) }
            if known != peers { peers = known }
        }
    }

    public func accept(handle: String, name: String) async throws {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        try await client.accept(handle: handle, name: trimmed.isEmpty ? nil : trimmed)
        await refresh()
    }

    public func peer(_ key: String) -> Peer? {
        peers.first { $0.key == key }
    }

    public func url(for id: PortID) -> URL? {
        guard let peer = peer(id.peer) else { return nil }
        return peer.pageURL(port: id.port, byName: opensByName, label: peer.hostLabel(among: peers))
    }

    /// The peer's title, with the start of its key when another peer has the
    /// same title.
    public func title(for peer: Peer) -> String {
        if peers.filter({ $0.title == peer.title }).count > 1 {
            return "\(peer.title) \(peer.key.prefix(8))"
        }
        return peer.title
    }
}

public enum PeerStatus: String, Sendable {
    case online, asleep, connecting, offline
    /// This app's pai-sho is starting, and has not listed its peers yet
    case starting
    /// This app's pai-sho is stopped
    case stopped
}
