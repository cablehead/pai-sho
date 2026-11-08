import PaiShoKit
import SwiftUI

/// One app for iPhone, iPad and Mac. The screens come from PaiShoKit.
///
/// On iOS the app runs the pai-sho process itself and talks to it over a
/// socket in its own container. On a Mac it talks to the daemon already
/// running there, at /tmp/pai-sho.sock, and opens pages at `<name>.pai-sho`
/// through that daemon's resolver.
///
/// Launch arguments, for driving the app by hand or from a test:
/// `-invite <invitation>` opens the add-a-peer sheet with it, as Paste would.
/// `-open <port>` opens the first port with that number once it is listed.
/// `-socket <path>` talks to another daemon (Mac only).
/// `-inviting YES` opens the Invite sheet (Mac only).
/// `-byName NO` opens pages at the address a port is bound at, for a daemon
/// without the `.pai-sho` resolver (Mac only).
@main
struct PaiShoApp: App {
    #if os(iOS)
    @Environment(\.scenePhase) private var phase
    @State private var wasInBackground = false
    @State private var process = InAppProcess()
    @State private var store = PeerStore(
        client: DaemonClient(socketPath: InAppProcess.socketName),
        opensByName: false
    )
    #else
    @State private var store = PeerStore(
        client: DaemonClient(socketPath: UserDefaults.standard.string(forKey: "socket") ?? "/tmp/pai-sho.sock"),
        opensByName: UserDefaults.standard.object(forKey: "byName") == nil
            || UserDefaults.standard.bool(forKey: "byName")
    )
    #endif

    var body: some Scene {
        WindowGroup {
            #if os(iOS)
            PaiShoView(store: store, invite: Self.launchInvite, open: Self.launchOpen) {
                ProcessSection(process: process, store: store)
            }
            .task { process.start() }
            // iOS reclaims an app's listening sockets while it is suspended,
            // so the local ports pai-sho bound are dead on return; the links
            // to peers are not. Open just the listeners again, at the same
            // numbers. If pai-sho can't be reached, start it again instead.
            .onChange(of: phase) { _, now in
                if now == .background { wasInBackground = true }
                if now == .active, wasInBackground {
                    wasInBackground = false
                    Task {
                        do {
                            try await store.client.rebind()
                        } catch {
                            await process.stop()
                            process.start()
                        }
                        await store.refresh()
                    }
                }
            }
            #else
            PaiShoView(
                store: store,
                invite: Self.launchInvite,
                open: Self.launchOpen,
                inviting: UserDefaults.standard.bool(forKey: "inviting")
            )
            .frame(minWidth: 720, minHeight: 480)
                #if DEBUG
                .task { await LaunchCheck.run(store: store) }
                #endif
            #endif
        }
        #if os(macOS)
        .defaultSize(width: 960, height: 640)
        #endif
    }

    private static var launchInvite: String? {
        UserDefaults.standard.string(forKey: "invite")
    }

    private static var launchOpen: UInt16? {
        UserDefaults.standard.string(forKey: "open").flatMap { UInt16($0) }
    }
}

#if os(iOS)
/// Start and Stop for the process running in this app.
struct ProcessSection: View {
    let process: InAppProcess
    let store: PeerStore

    var body: some View {
        Section {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(process.running ? "Running" : "Stopped")
                    if let problem = process.problem {
                        Text(problem)
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
                Spacer()
                if process.running {
                    Button("Stop", role: .destructive) {
                        Task {
                            await process.stop()
                            await store.refresh()
                        }
                    }
                } else {
                    Button("Start") {
                        process.start()
                        Task { await store.refresh() }
                    }
                }
            }
            .buttonStyle(.borderless)
            IconRow()
        } header: {
            Text("This phone")
        }
    }
}

/// The home-screen icons the app carries. `name` is the alternate icon's
/// asset name, nil for the main one.
struct AppIconChoice: Identifiable {
    let name: String?
    let title: String
    let preview: String
    var id: String { preview }

    static let all = [
        AppIconChoice(name: nil, title: "Board", preview: "IconPreview-Board"),
        AppIconChoice(name: "AppIcon-Drawn", title: "Drawn board", preview: "IconPreview-Drawn"),
        AppIconChoice(name: "AppIcon-Lotus", title: "Lotus tile", preview: "IconPreview-Lotus"),
        AppIconChoice(name: "AppIcon-Spirit", title: "Spirit", preview: "IconPreview-Spirit"),
    ]
}

/// Pick the home-screen icon. iOS confirms each change with its own notice.
struct IconRow: View {
    @State private var current = UIApplication.shared.alternateIconName

    private var chosen: AppIconChoice {
        AppIconChoice.all.first { $0.name == current } ?? AppIconChoice.all[0]
    }

    var body: some View {
        HStack {
            Text("Icon")
            Spacer()
            Menu {
                ForEach(AppIconChoice.all) { choice in
                    Button {
                        Task {
                            try? await UIApplication.shared.setAlternateIconName(choice.name)
                            current = UIApplication.shared.alternateIconName
                        }
                    } label: {
                        Label {
                            Text(choice.title)
                        } icon: {
                            Image(uiImage: (UIImage(named: choice.preview) ?? UIImage())
                                .withRenderingMode(.alwaysOriginal))
                        }
                    }
                }
            } label: {
                HStack(spacing: 8) {
                    Text(chosen.title)
                        .foregroundStyle(.secondary)
                    Image(chosen.preview)
                        .resizable()
                        .interpolation(.none)
                        .frame(width: 28, height: 28)
                        .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
                }
            }
            .accessibilityIdentifier("icon-menu")
        }
    }
}
#endif
