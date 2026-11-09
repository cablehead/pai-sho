import SwiftUI

// The one place UIKit and AppKit differ for the views this package wraps.
// WebView and QRScannerView implement `makeView` and `updateView` once and
// get both representable protocols from here.

#if os(iOS)
import UIKit
public typealias PlatformView = UIView

@MainActor
protocol PlatformViewRepresentable: UIViewRepresentable where UIViewType == Wrapped {
    associatedtype Wrapped: UIView
    func makeView(context: Context) -> Wrapped
    func updateView(_ view: Wrapped, context: Context)
    static func dismantleView(_ view: Wrapped, coordinator: Coordinator)
}

extension PlatformViewRepresentable {
    func makeUIView(context: Context) -> Wrapped { makeView(context: context) }
    func updateUIView(_ view: Wrapped, context: Context) { updateView(view, context: context) }
    static func dismantleUIView(_ view: Wrapped, coordinator: Coordinator) {
        dismantleView(view, coordinator: coordinator)
    }
    static func dismantleView(_ view: Wrapped, coordinator: Coordinator) {}
}

/// A web page's view, inside a controller of its own. A bare view only
/// counts the screen's edges as safe area; a controller's view also counts
/// the app's own bars, so WebKit sees the buttons over the page's bottom as
/// it sees the home bar. A page then scrolls under both and ends above both,
/// and keeps that around the keyboard, since WebKit does it itself.
final class PageController<Wrapped: UIView>: UIViewController {
    let wrapped: Wrapped

    init(_ wrapped: Wrapped) {
        self.wrapped = wrapped
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("not from a storyboard") }

    override func loadView() {
        view = UIView()
        view.backgroundColor = .clear
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        place()
    }

    /// A kept page's view moves to the newest controller showing it.
    func place() {
        guard isViewLoaded, wrapped.superview !== view else { return }
        wrapped.frame = view.bounds
        wrapped.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(wrapped)
    }
}

/// PlatformViewRepresentable, for a view wrapped in a PageController.
@MainActor
protocol PageViewRepresentable: UIViewControllerRepresentable where UIViewControllerType == PageController<Wrapped> {
    associatedtype Wrapped: UIView
    func makeView(context: Context) -> Wrapped
    func updateView(_ view: Wrapped, context: Context)
}

extension PageViewRepresentable {
    func makeUIViewController(context: Context) -> PageController<Wrapped> {
        PageController(makeView(context: context))
    }
    func updateUIViewController(_ controller: PageController<Wrapped>, context: Context) {
        controller.place()
        updateView(controller.wrapped, context: context)
    }
}
#elseif os(macOS)
import AppKit
public typealias PlatformView = NSView

@MainActor
protocol PlatformViewRepresentable: NSViewRepresentable where NSViewType == Wrapped {
    associatedtype Wrapped: NSView
    func makeView(context: Context) -> Wrapped
    func updateView(_ view: Wrapped, context: Context)
    static func dismantleView(_ view: Wrapped, coordinator: Coordinator)
}

extension PlatformViewRepresentable {
    func makeNSView(context: Context) -> Wrapped { makeView(context: context) }
    func updateNSView(_ view: Wrapped, context: Context) { updateView(view, context: context) }
    static func dismantleNSView(_ view: Wrapped, coordinator: Coordinator) {
        dismantleView(view, coordinator: coordinator)
    }
    static func dismantleView(_ view: Wrapped, coordinator: Coordinator) {}
}
#endif
