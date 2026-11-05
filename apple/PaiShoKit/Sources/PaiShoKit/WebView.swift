import SwiftUI
import WebKit

/// A peer's page. Loads `url` once, and again only when it changes.
struct WebView: PlatformViewRepresentable {
    var url: URL

    final class Coordinator {
        var loaded: URL?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeView(context: Context) -> WKWebView {
        let view = WKWebView()
        #if os(macOS)
        view.allowsMagnification = true
        #endif
        view.allowsBackForwardNavigationGestures = true
        return view
    }

    func updateView(_ view: WKWebView, context: Context) {
        guard context.coordinator.loaded != url else { return }
        context.coordinator.loaded = url
        view.load(URLRequest(url: url))
    }
}

/// The chosen port's page, with its address above it.
public struct PortPage: View {
    var title: String
    var url: URL

    public init(title: String, url: URL) {
        self.title = title
        self.url = url
    }

    public var body: some View {
        VStack(spacing: 0) {
            Text(url.absoluteString)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal)
                .padding(.vertical, 6)
            Divider()
            WebView(url: url)
                .id(url)
        }
        .navigationTitle(title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
    }
}
