import PaiShoKit
import SwiftUI

@main
struct PaiShoApp: App {
    @State private var process = InAppProcess()
    @State private var store = PeerStore(
        client: DaemonClient(socketPath: InAppProcess.socketName),
        opensByName: false
    )

    var body: some Scene {
        WindowGroup {
            PaiShoView(store: store, invite: Self.launchInvite) {
                ProcessSection(process: process, store: store)
            }
            .task { process.start() }
        }
    }

    /// `-invite <invitation>` on launch opens the add-a-peer sheet with it,
    /// as Paste would. For driving the app from `simctl launch`.
    private static var launchInvite: String? {
        UserDefaults.standard.string(forKey: "invite")
    }
}

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
