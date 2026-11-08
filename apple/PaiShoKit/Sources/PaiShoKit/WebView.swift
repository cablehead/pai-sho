import SwiftUI
import WebKit

/// A peer's page. Loads `url` once, and again only when it changes. If the
/// page fails over http it is tried over https; `shown` is the address that
/// is loading, and `failure` says why neither loaded.
struct WebView: PlatformViewRepresentable {
    var url: URL
    @Binding var shown: URL
    @Binding var failure: String?
    var onTitle: (String) -> Void = { _ in }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: URL?
        var parent: WebView

        init(_ parent: WebView) { self.parent = parent }

        func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
            parent.failure = nil
            if let title = view.title, !title.isEmpty { parent.onTitle(title) }
        }

        func webView(_ view: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            failed(view, error)
        }

        func webView(_ view: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            failed(view, error)
        }

        /// A peer's page served over https carries a certificate for some
        /// other name, or from a CA the phone does not trust. It is accepted at
        /// the one address this view opened, and nowhere else.
        func webView(_ view: WKWebView, respondTo challenge: URLAuthenticationChallenge) async
            -> (URLSession.AuthChallengeDisposition, URLCredential?)
        {
            let space = challenge.protectionSpace
            guard Peer.trustsCertificate(
                method: space.authenticationMethod,
                host: space.host,
                port: space.port,
                opened: parent.url
            ), let trust = space.serverTrust
            else { return (.performDefaultHandling, nil) }
            return (.useCredential, URLCredential(trust: trust))
        }

        private func failed(_ view: WKWebView, _ error: Error) {
            let error = error as NSError
            if error.domain == NSURLErrorDomain, error.code == NSURLErrorCancelled { return }
            if parent.shown == parent.url, let secure = Peer.secureRetry(for: parent.url) {
                parent.shown = secure
                view.load(URLRequest(url: secure))
                return
            }
            parent.failure = error.localizedDescription
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeView(context: Context) -> WKWebView {
        let view = WKWebView()
        #if os(macOS)
        view.allowsMagnification = true
        #endif
        view.allowsBackForwardNavigationGestures = true
        view.navigationDelegate = context.coordinator
        return view
    }

    func updateView(_ view: WKWebView, context: Context) {
        context.coordinator.parent = self
        guard context.coordinator.loaded != url else { return }
        context.coordinator.loaded = url
        view.load(URLRequest(url: url))
    }
}

/// The chosen port's page, with its address above it. `address` turns the
/// URL that is loading into what to show; by default, the URL itself.
public struct PortPage: View {
    var title: String
    var url: URL
    var address: (URL) -> String
    var onTitle: (String) -> Void
    @State private var shown: URL
    @State private var failure: String?

    public init(
        title: String,
        url: URL,
        onTitle: @escaping (String) -> Void = { _ in },
        address: @escaping (URL) -> String = { $0.absoluteString }
    ) {
        self.title = title
        self.url = url
        self.onTitle = onTitle
        self.address = address
        _shown = State(initialValue: url)
    }

    public var body: some View {
        VStack(spacing: 0) {
            Text(address(shown))
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal)
                .padding(.vertical, 6)
            Divider()
            WebView(url: url, shown: $shown, failure: $failure, onTitle: onTitle)
                .id(url)
                .overlay {
                    if let failure {
                        ContentUnavailableView(
                            "The page did not load",
                            systemImage: "exclamationmark.triangle",
                            description: Text(failure)
                        )
                        .background(.background)
                    }
                }
        }
        .onChange(of: url) {
            shown = url
            failure = nil
        }
        .navigationTitle(title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
    }
}
