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
    /// The ports whose site takes shares, by target id: what its manifest's
    /// share_target says. Remembered between shares, so the sheet lists them
    /// at once and checks again behind.
    var found: [String: ShareTarget] = ShareModel.remembered()
    /// Ports checked since the sheet opened, taking shares or not, and those
    /// whose check hasn't come back yet.
    var checked: Set<String> = []
    var pending: Set<String> = []
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
                let (bytes, name) = Self.unwrap(data, name: url.lastPathComponent)
                Task { @MainActor in self.files.append(Shared(name: name, data: bytes)) }
            }
        }
    }

    /// A screenshot shared straight from its editor is handed over as an
    /// image object, and its file representation is that object archived
    /// (NSKeyedArchiver), not the picture. Send the PNG it holds instead.
    nonisolated static func unwrap(_ data: Data, name: String) -> (Data, String) {
        guard data.starts(with: Data("bplist00".utf8)),
              let image = try? NSKeyedUnarchiver.unarchivedObject(ofClass: UIImage.self, from: data),
              let png = image.pngData()
        else { return (data, name) }
        return (png, (name as NSString).deletingPathExtension + ".png")
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

    /// Ask pai-sho for its peers until one is picked, and check each port
    /// once for a share target.
    func watch() async {
        let client = DaemonClient(socketPath: Self.socket)
        while sent == nil {
            if let peers = try? await client.list() {
                targets = peers.filter(\.online).flatMap { peer in
                    peer.ports.compactMap { port in
                        peer.address(port: port).map { Target(peer: peer.title, port: port, address: $0) }
                    }
                }
                for target in targets where !checked.contains(target.id) {
                    checked.insert(target.id)
                    pending.insert(target.id)
                    Task { await self.check(target) }
                }
            }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }

    /// The ports to offer: those whose site takes shares.
    var sharing: [Target] { targets.filter { found[$0.id] != nil } }

    /// Read the port's page, the manifest it links to, and its share_target,
    /// over http first, then https, as the app opens a port's page.
    func check(_ target: Target) async {
        var result: ShareTarget?
        for scheme in ["http", "https"] {
            guard let page = URL(string: "\(scheme)://\(target.address)/") else { continue }
            guard let (html, _) = try? await Uploader.session.data(from: page),
                  let manifest = ShareTarget.manifestURL(html: String(decoding: html, as: UTF8.self), page: page),
                  let (data, _) = try? await Uploader.session.data(from: manifest)
            else { continue }
            result = ShareTarget.from(manifest: data, at: manifest)
            break
        }
        found[target.id] = result
        pending.remove(target.id)
        Self.remember(found)
    }

    private static let rememberKey = "shareTargets"
    static func remembered() -> [String: ShareTarget] {
        guard let data = UserDefaults.standard.data(forKey: rememberKey) else { return [:] }
        return (try? JSONDecoder().decode([String: ShareTarget].self, from: data)) ?? [:]
    }
    static func remember(_ found: [String: ShareTarget]) {
        if let data = try? JSONEncoder().encode(found) { UserDefaults.standard.set(data, forKey: rememberKey) }
    }

    /// Send the files to `target`'s share target, as one form.
    func send(to target: Target) async {
        guard let share = found[target.id] else { return }
        sending = target
        problem = nil
        defer { sending = nil }
        let shared = files.map { file in
            (name: file.name, type: UTType(filenameExtension: (file.name as NSString).pathExtension)?.preferredMIMEType ?? "application/octet-stream", data: file.data)
        }
        let (body, contentType) = share.form(files: shared)
        var request = URLRequest(url: share.action)
        request.httpMethod = "POST"
        request.setValue(contentType, forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        guard let (_, response) = try? await Uploader.session.data(for: request),
              let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode)
        else {
            problem = "\(target.peer) : \(target.port) did not take the share."
            return
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
                } else if model.sharing.isEmpty, model.problem == nil {
                    if model.targets.isEmpty || !model.pending.isEmpty {
                        HStack {
                            ProgressView()
                            Text("Looking for ports that take shares...").foregroundStyle(.secondary)
                        }
                    } else {
                        Text("None of your peers' ports take shares. A site takes them by naming a share target in its web app manifest.")
                            .foregroundStyle(.secondary)
                    }
                } else {
                    Section {
                        ForEach(model.sharing) { target in
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
