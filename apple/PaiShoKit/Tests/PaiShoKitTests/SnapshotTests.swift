#if os(macOS)
import AppKit
import SwiftUI
import Testing
@testable import PaiShoKit

/// Pictures of views, for reviewing a change, not for asserting on. They run
/// only when SNAPSHOT_DIR is set, and draw off-screen: no window is shown and
/// nothing takes focus.
@MainActor @Suite struct Snapshots {
    static let dir = ProcessInfo.processInfo.environment["SNAPSHOT_DIR"]

    func render(_ view: some View, size: CGSize, to name: String) throws {
        guard let dir = Self.dir else { return }
        let host = NSHostingView(rootView: view.frame(width: size.width, height: size.height))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(
            contentRect: CGRect(x: -10000, y: -10000, width: size.width, height: size.height),
            styleMask: [.borderless], backing: .buffered, defer: false
        )
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let png = try #require(rep.representation(using: .png, properties: [:]))
        try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent(name))
    }

    @Test func aRefusedInvitation() throws {
        let store = PeerStore(
            client: DaemonClient(socketPath: "/nonexistent"),
            opensByName: true,
            defaults: UserDefaults(suiteName: "pai-sho-snapshots")!
        )
        try render(AcceptSheet(store: store, invite: "", refused: true), size: CGSize(width: 460, height: 420), to: "accept-refused.png")
    }

    @Test func keysTurnedAway() throws {
        let rows = [
            Refused(key: "3fa2c1d0abcdef", reason: "not authorized", count: 4, last: Date(timeIntervalSince1970: 1795022580)),
            Refused(key: "b6eef46d998877", reason: "enroll timeout", count: 1, last: Date(timeIntervalSince1970: 1795020000)),
        ]
        let list = List {
            Section {
                ForEach(rows) { RefusedRow(refused: $0) }
            } header: {
                Text("Turned away")
            } footer: {
                Text("Tried to connect without a live invitation of yours.")
            }
        }
        try render(list, size: CGSize(width: 420, height: 180), to: "turned-away.png")
    }
}
#endif
