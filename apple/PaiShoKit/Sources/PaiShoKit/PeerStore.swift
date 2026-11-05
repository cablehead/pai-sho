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

    public init(client: DaemonClient, opensByName: Bool) {
        self.client = client
        self.opensByName = opensByName
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
            problem = nil
            if next != peers { peers = next }
        } catch {
            loadedOnce = true
            problem = error.localizedDescription
            if !peers.isEmpty { peers = [] }
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
        peer(id.peer)?.pageURL(port: id.port, byName: opensByName)
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
