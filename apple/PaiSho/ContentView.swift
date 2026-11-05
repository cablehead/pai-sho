import SwiftUI

/// The phone screen. It starts the pai-sho process inside the app.
/// The socket is opened so other tools can talk to that process.
/// This screen does not use the socket.
struct ContentView: View {
    @State private var status = "Not started"
    @State private var started = false

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Pai Sho")
                .font(.largeTitle)
            Text(status)
                .font(.body)
            Button(started ? "Running" : "Start") {
                start()
            }
            .disabled(started)
            Button("Stop") {
                pai_sho_stop()
                status = "Stop requested"
                started = false
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func start() {
        let base = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("pai-sho", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        } catch {
            status = "Could not create \(base.path)"
            return
        }
        let socket = base.appendingPathComponent("pai-sho.sock").path
        let key = base.appendingPathComponent("key").path
        let code = socket.withCString { socket in
            key.withCString { key in
                "127.0.0.1".withCString { host in
                    pai_sho_start(host, socket, key)
                }
            }
        }
        if code == 0 {
            started = true
            status = "Process started"
        } else if code == -2 {
            started = true
            status = "Already running"
        } else {
            status = "Start failed (\(code))"
        }
    }
}
