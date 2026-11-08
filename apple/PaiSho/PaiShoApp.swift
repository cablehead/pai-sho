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
    /// The process was running when the app went to the background, so it
    /// starts again on return. A process the user stopped stays stopped.
    @State private var resumeOnReturn = false
    /// Saying goodbye and stopping, while the app is in the background
    @State private var sleeping: Task<Void, Never>?
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
            .onChange(of: process.running, initial: true) { _, now in
                store.processRunning = now
            }
            // iOS suspends the app soon after it goes to the background, and
            // closes its listening sockets. Before that, the process tells
            // each peer goodbye, so they stop dialing a phone that cannot
            // answer, and stops (ADR 0009). On return it starts again: saved
            // peers redial, which wakes them, and each port gets its saved
            // number back.
            .onChange(of: phase) { _, now in
                if now == .background, !wasInBackground {
                    wasInBackground = true
                    resumeOnReturn = process.running
                    if resumeOnReturn { sleeping = sleepInBackground() }
                }
                if now == .active, wasInBackground {
                    wasInBackground = false
                    let slept = sleeping
                    sleeping = nil
                    guard resumeOnReturn else { return }
                    Task {
                        await slept?.value
                        // Ran out of background time before it stopped: its
                        // ports are dead and it dials no one. Start afresh.
                        if process.running { await process.stop() }
                        process.start()
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

    #if os(iOS)
    /// Say goodbye to every peer and stop the process, inside a background
    /// task so iOS lets it finish. The goodbye takes about a second at most.
    private func sleepInBackground() -> Task<Void, Never> {
        let background = BackgroundTask()
        background.begin(name: "pai-sho goodbye")
        return Task {
            try? await store.client.sleep()
            await process.stop()
            background.end()
        }
    }
    #endif

    private static var launchInvite: String? {
        UserDefaults.standard.string(forKey: "invite")
    }

    private static var launchOpen: UInt16? {
        UserDefaults.standard.string(forKey: "open").flatMap { UInt16($0) }
    }
}

#if os(iOS)
/// Time iOS gives the app to finish something after it goes to the
/// background. When it runs out, iOS suspends the app anyway.
@MainActor
final class BackgroundTask {
    private var id = UIBackgroundTaskIdentifier.invalid

    func begin(name: String) {
        id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            self?.end()
        }
    }

    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }
}

/// Start and Stop for the process running in this app.
struct ProcessSection: View {
    let process: InAppProcess
    let store: PeerStore

    var body: some View {
        Section {
            // A switch, green while pai-sho runs.
            Toggle(isOn: Binding(
                get: { process.running },
                set: { on in
                    Task {
                        if on { process.start() } else { await process.stop() }
                        await store.refresh()
                    }
                }
            )) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(process.running ? "Running" : "Stopped")
                    if let problem = process.problem {
                        Text(problem)
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
            }
            .tint(.green)
            .accessibilityIdentifier("running")
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
