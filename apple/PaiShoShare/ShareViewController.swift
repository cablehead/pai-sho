import PaiShoKit
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Pai Sho in the share sheet. It runs pai-sho itself, as this device, with
/// the key and peers the app left in the keychain (`SharedState`), lists the
/// ports peers share with this device, and sends what was shared to the one
/// picked, as `POST /uploads/<name>`: the path an n2 board keeps files at.
final class ShareViewController: UIViewController {
    private let model = ShareModel()

    override func viewDidLoad() {
        super.viewDidLoad()
        model.finish = { [weak self] in
            pai_sho_stop()
            self?.extensionContext?.completeRequest(returningItems: nil)
        }
        let host = UIHostingController(rootView: ShareView(model: model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
        model.load(extensionContext?.inputItems as? [NSExtensionItem] ?? [])
        model.start()
    }
}

/// A file to send: its name and bytes.
struct Shared {
    var name: String
    var data: Data
}

/// A peer's port, as the sheet lists it.
struct Target: Identifiable, Hashable {
    var peer: String
    var port: UInt16
    /// Where it is bound here, `127.0.0.1:<n>`.
    var address: String
    var id: String { "\(peer):\(port)" }
}

@MainActor
@Observable
final class ShareModel {
    var files: [Shared] = []
    var targets: [Target] = []
    var problem: String?
    var sending: Target?
    var sent: Target?
    var finish: () -> Void = {}

    static let socket = "pai-sho.sock"

    /// Read every image or file that was shared.
    func load(_ items: [NSExtensionItem]) {
        for provider in items.flatMap({ $0.attachments ?? [] }) {
            let type = [UTType.image, .data].first { provider.hasItemConformingToTypeIdentifier($0.identifier) }
            guard let type else { continue }
            provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                guard let url, let data = try? Data(contentsOf: url) else { return }
                let name = url.lastPathComponent
                Task { @MainActor in self.files.append(Shared(name: name, data: data)) }
            }
        }
    }

    /// Start pai-sho with the app's key, then list the ports peers share as
    /// they come online.
    func start() {
        let fm = FileManager.default
        let base = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("pai-sho", isDirectory: true)
        guard SharedState.restore(to: base) else {
            problem = "Open Pai Sho once first, so this can use its peers."
            return
        }
        guard fm.changeCurrentDirectoryPath(base.path) else {
            problem = "Could not change to \(base.path)"
            return
        }
        let key = base.appendingPathComponent("key").path
        let code = "127.0.0.1".withCString { host in
            Self.socket.withCString { socket in
                key.withCString { pai_sho_start(host, socket, $0) }
            }
        }
        guard code == 0 || code == -2 else {
            problem = "pai_sho_start returned \(code)"
            return
        }
        Task { await watch() }
    }

    /// Ask pai-sho for its peers until one is picked.
    func watch() async {
        let client = DaemonClient(socketPath: Self.socket)
        while sent == nil {
            if let peers = try? await client.list() {
                targets = peers.filter(\.online).flatMap { peer in
                    peer.ports.compactMap { port in
                        peer.address(port: port).map { Target(peer: peer.title, port: port, address: $0) }
                    }
                }
            }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }

    /// Send each file to `target`, over http first, then https, as the app
    /// opens a port's page.
    func send(to target: Target) async {
        sending = target
        problem = nil
        defer { sending = nil }
        for file in files {
            let name = file.name.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? "file"
            var done = false
            for scheme in ["http", "https"] {
                guard let url = URL(string: "\(scheme)://\(target.address)/uploads/\(name)") else { continue }
                var request = URLRequest(url: url)
                request.httpMethod = "POST"
                request.httpBody = file.data
                if let (_, response) = try? await Uploader.session.data(for: request),
                   let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) {
                    done = true
                    break
                }
            }
            guard done else {
                problem = "\(target.peer) : \(target.port) did not take \(file.name)."
                return
            }
        }
        sent = target
        try? await Task.sleep(for: .seconds(1))
        finish()
    }
}

/// The session uploads go through. It accepts any certificate from
/// 127.0.0.1 alone: that address is the local end of pai-sho's encrypted
/// link to a known peer key, so a certificate's name adds nothing there.
final class Uploader: NSObject, URLSessionDelegate {
    static let session = URLSession(configuration: .ephemeral, delegate: Uploader(), delegateQueue: nil)

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge)
        async -> (URLSession.AuthChallengeDisposition, URLCredential?)
    {
        let space = challenge.protectionSpace
        guard space.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              space.host == "127.0.0.1", let trust = space.serverTrust
        else { return (.performDefaultHandling, nil) }
        return (.useCredential, URLCredential(trust: trust))
    }
}

struct ShareView: View {
    let model: ShareModel

    var body: some View {
        NavigationStack {
            List {
                if let problem = model.problem {
                    Text(problem).foregroundStyle(.red)
                }
                if let sent = model.sent {
                    Label("Sent to \(sent.peer) : " + String(sent.port), systemImage: "checkmark.circle.fill")
                } else if model.targets.isEmpty, model.problem == nil {
                    HStack {
                        ProgressView()
                        Text("Connecting to your peers...").foregroundStyle(.secondary)
                    }
                } else {
                    Section {
                        ForEach(model.targets) { target in
                            Button {
                                Task { await model.send(to: target) }
                            } label: {
                                HStack {
                                    Text(target.peer)
                                    Spacer()
                                    if model.sending == target {
                                        ProgressView()
                                    } else {
                                        Text(":" + String(target.port)).monospacedDigit().foregroundStyle(.secondary)
                                    }
                                }
                            }
                            .disabled(model.sending != nil || model.files.isEmpty)
                        }
                    } header: {
                        Text(model.files.count == 1 ? "Send \(model.files[0].name) to" : "Send \(model.files.count) files to")
                    }
                }
            }
            .navigationTitle("Pai Sho")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.finish() }
                }
            }
        }
    }
}
