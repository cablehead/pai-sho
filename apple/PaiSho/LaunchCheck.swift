#if DEBUG && os(macOS)
import AppKit
import PaiShoKit
import ScreenCaptureKit

/// Debug builds only: lets a script check the Mac app with no clicks and no
/// screen recording permission.
///
/// `-accept <invitation>` (with `-name <name>`) takes it up, the same call
/// the Accept button makes. `-snapshot <path>` writes a PNG of the app's
/// window after `-snapshotAfter <seconds>` (default 10), then quits.
enum LaunchCheck {
    @MainActor
    static func run(store: PeerStore) async {
        let defaults = UserDefaults.standard
        if let invite = defaults.string(forKey: "accept") {
            do {
                try await store.accept(handle: invite, name: defaults.string(forKey: "name") ?? "")
                say("accepted")
            } catch {
                say("accept failed: \(error.localizedDescription)")
            }
        }
        guard let path = defaults.string(forKey: "snapshot") else { return }
        let after = defaults.object(forKey: "snapshotAfter") as? Double
            ?? Double(defaults.string(forKey: "snapshotAfter") ?? "") ?? 10
        say("snapshot in \(after)s")
        try? await Task.sleep(for: .seconds(after))
        do {
            try await snapshot(to: URL(fileURLWithPath: path))
            say("wrote \(path)")
        } catch {
            say("snapshot failed: \(error)")
        }
        exit(0)
    }

    /// The app's visible windows (the main window, and a sheet when one is
    /// open, as `<path>-1.png`), through ScreenCaptureKit's current-process
    /// content, which needs no screen recording permission.
    @MainActor
    private static func snapshot(to url: URL) async throws {
        guard #available(macOS 14.4, *) else { throw CheckError("needs macOS 14.4") }
        let numbers = NSApp.windows.filter(\.isVisible).map { CGWindowID($0.windowNumber) }
        guard !numbers.isEmpty else { throw CheckError("no window") }
        let content = try await SCShareableContent.currentProcess
        let scale = NSApp.windows.first?.backingScaleFactor ?? 2
        for (index, number) in numbers.enumerated() {
            guard let window = content.windows.first(where: { $0.windowID == number }) else { continue }
            let config = SCStreamConfiguration()
            config.width = Int(window.frame.width * scale)
            config.height = Int(window.frame.height * scale)
            let image = try await SCScreenshotManager.captureImage(
                contentFilter: SCContentFilter(desktopIndependentWindow: window),
                configuration: config
            )
            guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
                throw CheckError("could not encode PNG")
            }
            let target = index == 0 ? url
                : url.deletingPathExtension().appendingPathExtension("\(index)").appendingPathExtension("png")
            try data.write(to: target)
        }
    }

    private static func say(_ line: String) {
        FileHandle.standardError.write(Data("LaunchCheck: \(line)\n".utf8))
    }

    struct CheckError: Error, CustomStringConvertible {
        var description: String
        init(_ description: String) { self.description = description }
    }
}
#endif
