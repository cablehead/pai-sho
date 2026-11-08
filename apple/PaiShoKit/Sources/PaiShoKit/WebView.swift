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
    /// Back, forward and reload for the page's controls.
    var nav: PageNav

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        var loaded: URL?
        var parent: WebView
        /// Retries left while nothing is listening yet: on iPhone, pai-sho
        /// is still binding its ports again after the app comes back.
        var retries = Coordinator.maxRetries
        static let maxRetries = 10

        init(_ parent: WebView) { self.parent = parent }

        func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
            parent.failure = nil
            #if os(iOS)
            // Clear only until the page paints (no white flash). Once it has,
            // let the page's own background fill the strip behind the clock.
            view.isOpaque = true
            view.backgroundColor = nil
            view.scrollView.backgroundColor = nil
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

        /// A link that opens a new tab (`target="_blank"`, as n2's card links
        /// do). There are no tabs here: a peer's page opens in this view, and
        /// anything else in the system browser. Without this, WebKit asks for
        /// a new window, gets none, and the tap does nothing.
        func webView(
            _ view: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for action: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            guard let url = action.request.url else { return nil }
            if Coordinator.opensHere(url, from: view.url) {
                view.load(action.request)
            } else {
                #if os(iOS)
                UIApplication.shared.open(url)
                #else
                NSWorkspace.shared.open(url)
                #endif
            }
            return nil
        }

        /// A peer's page: the same site, or any port pai-sho binds locally
        /// (`.localhost`, 127.0.0.1, or a `.pai-sho` name). Outside the app
        /// those addresses lead nowhere.
        static func opensHere(_ url: URL, from current: URL?) -> Bool {
            guard let host = url.host?.lowercased() else { return false }
            if let current, current.host?.lowercased() == host { return true }
            return host == "localhost" || host.hasSuffix(".localhost") || host == "127.0.0.1"
                || host.hasSuffix(".pai-sho")
        }

        /// The address a failed load was for.
        static func failedURL(_ error: NSError) -> URL? {
            (error.userInfo[NSURLErrorFailingURLErrorKey] as? URL)
                ?? (error.userInfo[NSURLErrorFailingURLStringErrorKey] as? String).flatMap(URL.init(string:))
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
                // The page that failed, not the port's front page: coming
                // back to the app, a page reloading while pai-sho restarts
                // must come back as itself.
                let url = Coordinator.failedURL(error) ?? view.url ?? parent.url
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
        // A port's page is kept while the app runs, as a browser keeps a tab:
        // out to the main screen and back in finds it where you left it, not
        // reloaded at the port's front page.
        if let kept = Tabs.views[url] {
            context.coordinator.loaded = url
            nav.attach(kept)
            kept.navigationDelegate = context.coordinator
            kept.uiDelegate = context.coordinator
            return kept
        }
        let view = WKWebView()
        Tabs.views[url] = view
        #if os(macOS)
        view.allowsMagnification = true
        #else
        // Without this the view paints white before the page arrives, a
        // flash on a dark page. Clear, it shows the app's own background.
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        #endif
        // A browser's own swipes: back and forward through the page's history.
        view.allowsBackForwardNavigationGestures = true
        nav.attach(view)
        view.navigationDelegate = context.coordinator
        view.uiDelegate = context.coordinator
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
    @State private var nav = PageNav()

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
            WebView(url: url, shown: $shown, failure: $failure, onTitle: onTitle, nav: nav)
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
        // No header on a phone: the clock, then the page, and a browser's
        // controls at the bottom. The page's back and forward go through its
        // own history, as in any browser; the main screen is a button of its
        // own, so getting out never means walking back through the history.
        .toolbar(.hidden, for: .navigationBar)
        .toolbar {
            ToolbarItemGroup(placement: .bottomBar) {
                Button(action: onLeave) {
                    Label("Pai Sho", systemImage: "house")
                }
                .accessibilityIdentifier("peers")
                Spacer()
                Button { nav.view?.goBack() } label: {
                    Label("Back", systemImage: "chevron.backward")
                }
                .disabled(!nav.canGoBack)
                .accessibilityIdentifier("back")
                Button { nav.view?.goForward() } label: {
                    Label("Forward", systemImage: "chevron.forward")
                }
                .disabled(!nav.canGoForward)
                .accessibilityIdentifier("forward")
                Spacer()
                Button { nav.view?.reload() } label: {
                    Label("Reload", systemImage: "arrow.clockwise")
                }
                .accessibilityIdentifier("reload")
            }
        }
        #else
        .navigationTitle(title)
        #endif
    }
}

/// What a page's controls need from its web view: whether it can go back or
/// forward, and the view to tell.
@MainActor
@Observable
final class PageNav {
    @ObservationIgnored weak var view: WKWebView?
    var canGoBack = false
    var canGoForward = false
    @ObservationIgnored private var watches: [NSKeyValueObservation] = []

    func attach(_ view: WKWebView) {
        self.view = view
        watches = [
            view.observe(\.canGoBack, options: [.initial, .new]) { [weak self] view, _ in
                MainActor.assumeIsolated { self?.canGoBack = view.canGoBack }
            },
            view.observe(\.canGoForward, options: [.initial, .new]) { [weak self] view, _ in
                MainActor.assumeIsolated { self?.canGoForward = view.canGoForward }
            },
        ]
    }
}

/// The page open at each port's address, kept for the app's life.
@MainActor
enum Tabs {
    static var views: [URL: WKWebView] = [:]
}
