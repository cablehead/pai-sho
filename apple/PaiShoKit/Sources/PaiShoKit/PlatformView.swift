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
