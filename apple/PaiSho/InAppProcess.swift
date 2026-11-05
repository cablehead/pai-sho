import Foundation
import Observation

/// The pai-sho process, run inside this app by `pai_sho_start`.
///
/// Its key and state live in Application Support/pai-sho. Its control socket
/// is there too, but is named relative to the working directory, because a
/// socket path cannot be longer than 103 bytes and an app's container path
/// can be (in the simulator it is). The app makes that directory its working
/// directory before starting the process.
@MainActor
@Observable
final class InAppProcess {
    static let socketName = "pai-sho.sock"

    private(set) var running = false
    private(set) var problem: String?

    func start() {
        guard !running else { return }
        let fm = FileManager.default
        let base = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("pai-sho", isDirectory: true)
        do {
            try fm.createDirectory(at: base, withIntermediateDirectories: true)
        } catch {
            problem = "Could not create \(base.path): \(error.localizedDescription)"
            return
        }
        guard fm.changeCurrentDirectoryPath(base.path) else {
            problem = "Could not change to \(base.path)"
            return
        }
        let key = base.appendingPathComponent("key").path
        let code = "127.0.0.1".withCString { host in
            Self.socketName.withCString { socket in
                key.withCString { key in
                    pai_sho_start(host, socket, key)
                }
            }
        }
        switch code {
        case 0, -2:
            running = true
            problem = nil
        default:
            problem = "pai_sho_start returned \(code)"
        }
    }

    /// Ask the process to stop and wait, up to five seconds, until it has.
    func stop() async {
        pai_sho_stop()
        for _ in 0..<50 where pai_sho_running() != 0 {
            try? await Task.sleep(for: .milliseconds(100))
        }
        running = pai_sho_running() != 0
        if running { problem = "Still stopping" }
    }
}
