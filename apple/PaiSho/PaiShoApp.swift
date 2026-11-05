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
/// `-byName NO` opens pages at the address a port is bound at, for a daemon
/// without the `.pai-sho` resolver (Mac only).
@main
struct PaiShoApp: App {
    #if os(iOS)
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
            #else
            PaiShoView(store: store, invite: Self.launchInvite, open: Self.launchOpen)
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
        } header: {
            Text("This phone")
        }
    }
}
#endif
