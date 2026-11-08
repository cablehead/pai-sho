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
    /// Leave the page. Nothing in the view calls it now: on iPhone the
    /// navigation's own swipe leaves the page (PageSwipe).
    var onLeave: () -> Void = {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: URL?
        var parent: WebView
        /// Retries left while nothing is listening yet: on iPhone, pai-sho
        /// is still binding its ports again after the app comes back.
        var retries = Coordinator.maxRetries
        static let maxRetries = 10

        init(_ parent: WebView) { self.parent = parent }

        #if os(iOS)
        var swipe: PageSwipe?
        #endif

        func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
            parent.failure = nil
            #if os(iOS)
            // Clear only until the page paints (no white flash). Once it has,
            // let the page's own background fill the strip behind the clock.
            view.isOpaque = true
            view.backgroundColor = nil
            view.scrollView.backgroundColor = nil
            swipe?.attach()
            #endif
            retries = Coordinator.maxRetries
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

        /// The local port refused or dropped the connection: nothing is
        /// listening there right now.
        static func nothingListening(_ error: NSError) -> Bool {
            error.domain == NSURLErrorDomain
                && [NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost].contains(error.code)
        }

        private func failed(_ view: WKWebView, _ error: Error) {
            let error = error as NSError
            if error.domain == NSURLErrorDomain, error.code == NSURLErrorCancelled { return }
            if Coordinator.nothingListening(error), retries > 0 {
                retries -= 1
                let url = parent.url
                parent.shown = url
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak view] in
                    view?.load(URLRequest(url: url))
                }
                return
            }
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
        #else
        // Without this the view paints white before the page arrives, a
        // flash on a dark page. Clear, it shows the app's own background.
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        #endif
        #if os(macOS)
        view.allowsBackForwardNavigationGestures = true
        #else
        // Edge swipes slide the page under the finger: right for back (an n2
        // thread to its list, then out to the main screen), left for forward.
        let swipe = PageSwipe(view)
        context.coordinator.swipe = swipe
        DispatchQueue.main.async { swipe.attach() }
        #endif
        view.navigationDelegate = context.coordinator
        return view
    }

    func updateView(_ view: WKWebView, context: Context) {
        context.coordinator.parent = self
        guard context.coordinator.loaded != url else { return }
        context.coordinator.loaded = url
        context.coordinator.retries = Coordinator.maxRetries
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
    var onLeave: () -> Void
    @State private var shown: URL
    @State private var failure: String?

    public init(
        title: String,
        url: URL,
        onTitle: @escaping (String) -> Void = { _ in },
        onLeave: @escaping () -> Void = {},
        address: @escaping (URL) -> String = { $0.absoluteString }
    ) {
        self.title = title
        self.url = url
        self.onTitle = onTitle
        self.onLeave = onLeave
        self.address = address
        _shown = State(initialValue: url)
    }

    public var body: some View {
        VStack(spacing: 0) {
            #if os(macOS)
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
            #endif
            WebView(url: url, shown: $shown, failure: $failure, onTitle: onTitle, onLeave: onLeave)
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
                #if os(iOS)
                // On a phone the page is the point: it fills the screen, its
                // background running up behind the clock and down to the
                // bottom edge. The web view still insets the content itself,
                // so nothing sits under the status bar.
                .ignoresSafeArea(.container, edges: [.top, .bottom])
                #endif
        }
        .onChange(of: url) {
            shown = url
            failure = nil
        }
        #if os(iOS)
        // No header on a phone: the clock, then the page. Swipe right to go
        // back, in the page and then out of it.
        .toolbar(.hidden, for: .navigationBar)
        #else
        .navigationTitle(title)
        #endif
    }
}
