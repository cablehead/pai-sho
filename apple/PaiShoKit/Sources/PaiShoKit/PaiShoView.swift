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
    @State private var inviting: Bool

    enum Sheet: Identifiable {
        case scan
        case accept(String)
        case invite

        var id: String {
            switch self {
            case .scan: return "scan"
            case .accept(let invite): return "accept:" + invite
            case .invite: return "invite"
            }
        }
    }

    /// `invite`, when given, opens the add-a-peer sheet with it filled in, as
    /// Paste would. `open`, when given, opens the first port with that number
    /// once a peer lists it. `inviting` opens the Invite sheet (macOS).
    public init(
        store: PeerStore,
        invite: String? = nil,
        open: UInt16? = nil,
        inviting: Bool = false,
        @ViewBuilder controls: () -> Controls
    ) {
        self.store = store
        self.controls = controls()
        _invite = State(initialValue: invite)
        _open = State(initialValue: open)
        _inviting = State(initialValue: inviting)
    }

    public var body: some View {
        NavigationSplitView {
            PeerList(store: store, selection: $selection, controls: controls)
                .navigationTitle("Pai Sho")
                #if os(macOS)
                .navigationSplitViewColumnWidth(min: 220, ideal: 260)
                #endif
                .toolbar {
                    // The phone scans an invitation with its camera; the Mac
                    // pastes one, since it has no camera to point at a code.
                    ToolbarItemGroup(placement: .primaryAction) {
                        #if os(iOS)
                        Button {
                            sheet = .scan
                        } label: {
                            Label("Scan", systemImage: "qrcode.viewfinder")
                        }
                        #else
                        Button {
                            sheet = .invite
                        } label: {
                            Label("Invite", systemImage: "qrcode")
                        }
                        PasteButton(payloadType: String.self) { strings in
                            let text = strings.first ?? ""
                            Task { @MainActor in
                                sheet = .accept(text.trimmingCharacters(in: .whitespacesAndNewlines))
                            }
                        }
                        #endif
                    }
                }
        } detail: {
            if let selection, let peer = store.peer(selection.peer) {
                // While the port is down (pai-sho restarting as the app comes
                // back, or the peer reconnecting), keep the page as it was,
                // at the address it comes back at.
                let live = store.url(for: selection)
                if let url = live ?? store.reached[selection] {
                    PortPage(
                        title: "\(store.title(for: peer)) : \(selection.port)",
                        url: url,
                        onTitle: { store.remember(title: $0, for: selection) },
                        onLeave: { self.selection = nil }
                    ) { shown in
                        peer.displayURL(shown, port: selection.port)
                    }
                    .overlay(alignment: .top) {
                        if live == nil {
                            Reconnecting(to: store.title(for: peer))
                        }
                    }
                    .animation(.default, value: live == nil)
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
            case .invite:
                #if os(macOS)
                InviteSheet(client: store.client)
                #endif
            }
        }
        .task { await store.watch() }
        .task {
            // On macOS a sheet presented while the window is still coming up
            // is sometimes dropped, so give the window a moment first.
            guard invite != nil || inviting else { return }
            try? await Task.sleep(for: .milliseconds(500))
            if let invite {
                self.invite = nil
                sheet = .accept(invite)
            } else {
                inviting = false
                sheet = .invite
            }
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
    public init(store: PeerStore, invite: String? = nil, open: UInt16? = nil, inviting: Bool = false) {
        self.init(store: store, invite: invite, open: open, inviting: inviting) { EmptyView() }
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
                            PortRow(port: port, bound: peer.address(port: port) != nil, title: store.pageTitles[id])
                        }
                        .accessibilityIdentifier("port-\(port)")
                    }
                } header: {
                    PeerHeader(title: store.title(for: peer), status: store.status(of: peer))
                }
            }
        }
        .overlay {
            if store.peers.isEmpty {
                ContentUnavailableView {
                    Label("No peers", systemImage: "person.2")
                } description: {
                    if !store.reachable, store.processRunning == true {
                        // The app's own pai-sho is still coming up.
                        Text("Starting pai-sho...")
                    } else if let problem = store.problem {
                        Text(problem)
                    } else if store.loadedOnce {
                        #if os(iOS)
                        Text("Scan an invitation.")
                        #else
                        Text("Paste an invitation.")
                        #endif
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
    var status: PeerStatus

    private var dot: Color {
        switch status {
        case .online: return .green
        case .connecting, .starting: return .orange
        default: return Color.secondary.opacity(0.5)
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(dot)
                .frame(width: 8, height: 8)
            Text(title)
            Spacer()
            Text(status == .connecting || status == .starting ? "\(status.rawValue)..." : status.rawValue)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

/// A small note over a page whose port is down for now.
struct Reconnecting: View {
    var to: String
    init(to: String) { self.to = to }

    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            Text("Reconnecting to \(to)...")
                .font(.footnote)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.regularMaterial, in: Capsule())
        .padding(.top, 8)
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}

/// A port, and the title its page had when last opened, greyed out until it
/// is bound here. The local address it listens at says nothing to the user,
/// so it isn't shown.
struct PortRow: View {
    var port: UInt16
    var bound: Bool
    var title: String?

    var body: some View {
        HStack {
            Text(String(port))
                .font(.body.monospacedDigit())
                .foregroundStyle(bound ? .primary : .tertiary)
            Spacer()
            if let title {
                Text(title)
                    .font(.callout)
                    .foregroundStyle(bound ? .secondary : .tertiary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
        }
    }
}
