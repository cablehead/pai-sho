import SwiftUI

/// The whole app screen: peers and their ports on one side, the chosen port's
/// page on the other. On a phone the two stack and a port pushes its page.
/// `controls` sits above the peers; the iOS app puts Start and Stop there.
public struct PaiShoView<Controls: View>: View {
    let store: PeerStore
    let controls: Controls
    @State private var selection: PortID?
    @State private var sheet: Sheet?
    @State private var open: UInt16?
    @State private var invite: String?

    enum Sheet: Identifiable {
        case scan
        case accept(String)

        var id: String {
            switch self {
            case .scan: return "scan"
            case .accept(let invite): return "accept:" + invite
            }
        }
    }

    /// `invite`, when given, opens the add-a-peer sheet with it filled in, as
    /// Paste would. `open`, when given, opens the first port with that number
    /// once a peer lists it.
    public init(
        store: PeerStore,
        invite: String? = nil,
        open: UInt16? = nil,
        @ViewBuilder controls: () -> Controls
    ) {
        self.store = store
        self.controls = controls()
        _invite = State(initialValue: invite)
        _open = State(initialValue: open)
    }

    public var body: some View {
        NavigationSplitView {
            PeerList(store: store, selection: $selection, controls: controls)
                .navigationTitle("Pai Sho")
                #if os(macOS)
                .navigationSplitViewColumnWidth(min: 220, ideal: 260)
                #endif
                .toolbar {
                    ToolbarItemGroup(placement: .primaryAction) {
                        Button {
                            sheet = .scan
                        } label: {
                            Label("Scan", systemImage: "qrcode.viewfinder")
                        }
                        PasteButton(payloadType: String.self) { strings in
                            let text = strings.first ?? ""
                            Task { @MainActor in
                                sheet = .accept(text.trimmingCharacters(in: .whitespacesAndNewlines))
                            }
                        }
                    }
                }
        } detail: {
            if let selection, let peer = store.peer(selection.peer) {
                if let url = store.url(for: selection) {
                    PortPage(title: "\(store.title(for: peer)) : \(selection.port)", url: url)
                } else {
                    ContentUnavailableView(
                        "Not reachable yet",
                        systemImage: "network.slash",
                        description: Text("Port \(selection.port) is not bound here yet. It is once \(store.title(for: peer)) is online.")
                    )
                }
            } else {
                ContentUnavailableView(
                    "Choose a port",
                    systemImage: "network",
                    description: Text("The page opens here.")
                )
            }
        }
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .scan:
                ScanSheet { code in self.sheet = .accept(code) }
            case .accept(let invite):
                AcceptSheet(store: store, invite: invite)
            }
        }
        .task { await store.watch() }
        .task {
            // On macOS a sheet presented while the window is still coming up
            // is sometimes dropped, so give the window a moment first.
            guard let invite else { return }
            self.invite = nil
            try? await Task.sleep(for: .milliseconds(500))
            sheet = .accept(invite)
        }
        .onChange(of: store.peers) {
            if let port = open,
               let peer = store.peers.first(where: { $0.ports.contains(port) && $0.address(port: port) != nil }) {
                open = nil
                selection = PortID(peer: peer.key, port: port)
            }
            if let selection, store.peer(selection.peer)?.ports.contains(selection.port) != true {
                self.selection = nil
            }
        }
    }
}

extension PaiShoView where Controls == EmptyView {
    public init(store: PeerStore, invite: String? = nil, open: UInt16? = nil) {
        self.init(store: store, invite: invite, open: open) { EmptyView() }
    }
}

/// Each peer, online or not, and the ports it exposes to us.
struct PeerList<Controls: View>: View {
    let store: PeerStore
    @Binding var selection: PortID?
    let controls: Controls

    var body: some View {
        List(selection: $selection) {
            controls
            ForEach(store.peers) { peer in
                Section {
                    if peer.ports.isEmpty {
                        Text("No ports")
                            .foregroundStyle(.tertiary)
                    }
                    ForEach(peer.ports, id: \.self) { port in
                        let id = PortID(peer: peer.key, port: port)
                        NavigationLink(value: id) {
                            PortRow(port: port, address: peer.address(port: port))
                        }
                        .accessibilityIdentifier("port-\(port)")
                    }
                } header: {
                    PeerHeader(title: store.title(for: peer), online: peer.online)
                }
            }
        }
        .overlay {
            if store.peers.isEmpty {
                ContentUnavailableView {
                    Label("No peers", systemImage: "person.2")
                } description: {
                    if let problem = store.problem {
                        Text(problem)
                    } else if store.loadedOnce {
                        Text("Scan an invitation, or paste one.")
                    } else {
                        Text("Looking for pai-sho...")
                    }
                }
            }
        }
    }
}

struct PeerHeader: View {
    var title: String
    var online: Bool

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(online ? Color.green : Color.secondary.opacity(0.5))
                .frame(width: 8, height: 8)
            Text(title)
            Spacer()
            Text(online ? "online" : "offline")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

struct PortRow: View {
    var port: UInt16
    var address: String?

    var body: some View {
        HStack {
            Text(String(port))
                .font(.body.monospacedDigit())
            Spacer()
            if let address {
                Text(address)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            } else {
                Text("not bound")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}
