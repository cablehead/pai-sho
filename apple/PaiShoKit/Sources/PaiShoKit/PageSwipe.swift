#if os(iOS)
import UIKit
import UIKit.UIGestureRecognizerSubclass
import WebKit

/// Edge swipes on a peer's page that slide it under the finger, as Safari's
/// do. Pull far enough, or flick, and it carries on to the next page; let go
/// early and it falls back. Swiping right shows the page you came from
/// underneath, from a snapshot taken while you were on it. With no history
/// left, the navigation's own swipe slides the page away to the main screen.
///
/// WebKit's own swipe (`allowsBackForwardNavigationGestures`) never began
/// here, inside SwiftUI's navigation, so the slide is done by hand.
@MainActor
final class PageSwipe: NSObject, UIGestureRecognizerDelegate {
    private weak var view: WKWebView?
    private let back = UIScreenEdgePanGestureRecognizer()
    private let forward = UIScreenEdgePanGestureRecognizer()
    private weak var navigation: UINavigationController?
    private var watches: [NSKeyValueObservation] = []
    /// How each page in the history looked when last seen.
    private var looks: [WKBackForwardListItem: UIView] = [:]
    /// The snapshots on screen during a slide: the page being left, and the
    /// one coming in.
    private var leaving: UIView?
    private var coming: UIView?

    init(_ view: WKWebView) {
        self.view = view
        super.init()
        back.edges = .left
        forward.edges = .right
        for pan in [back, forward] {
            pan.addTarget(self, action: #selector(slide(_:)))
            view.addGestureRecognizer(pan)
            // The page's scrolling waits for the edge, or it takes the touch.
            view.scrollView.panGestureRecognizer.require(toFail: pan)
        }
        // A look at the page as a touch lands, before a tap navigates away.
        view.addGestureRecognizer(TouchDown { [weak self] in self?.remember() })
        watches = [
            view.observe(\.canGoBack) { [weak self] _, _ in MainActor.assumeIsolated { self?.gate() } },
            view.observe(\.canGoForward) { [weak self] _, _ in MainActor.assumeIsolated { self?.gate() } },
        ]
        gate()
    }

    /// Find the navigation once the page is on screen.
    func attach() {
        guard let view, navigation == nil else { gate(); return }
        let parent = sequence(first: view.next, next: { $0?.next })
            .first { $0 is UINavigationController } as? UINavigationController
        guard let parent, let pop = parent.interactivePopGestureRecognizer else { return }
        navigation = parent
        // SwiftUI turns the navigation's swipe off with its bar hidden.
        pop.delegate = self
        view.scrollView.panGestureRecognizer.require(toFail: pop)
        gate()
    }

    /// Back slides within the page while it has history, and the navigation's
    /// swipe takes over once it has none. Only one of them may hear the edge.
    private func gate() {
        guard let view else { return }
        back.isEnabled = view.canGoBack
        forward.isEnabled = view.canGoForward
        navigation?.interactivePopGestureRecognizer?.isEnabled = !view.canGoBack
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        (navigation?.viewControllers.count ?? 0) > 1
    }

    private func remember() {
        guard let view, let item = view.backForwardList.currentItem,
              let look = view.snapshotView(afterScreenUpdates: false) else { return }
        looks[item] = look
        let kept = Set(view.backForwardList.backList + view.backForwardList.forwardList + [item])
        looks = looks.filter { kept.contains($0.key) }
    }

    @objc private func slide(_ pan: UIScreenEdgePanGestureRecognizer) {
        guard let view else { return }
        let isBack = pan === back
        let width = view.bounds.width
        let pulled = min(max(isBack ? pan.translation(in: view).x : -pan.translation(in: view).x, 0), width)
        switch pan.state {
        case .began:
            remember()
            let item = isBack ? view.backForwardList.backItem : view.backForwardList.forwardItem
            let incoming = backed(item.flatMap { looks[$0] }, in: view)
            let outgoing = backed(view.snapshotView(afterScreenUpdates: false), in: view)
            // Back: the page slides off to the right, the one before it
            // waiting underneath. Forward: the next one slides in over it.
            let top = isBack ? outgoing : incoming
            top.layer.shadowColor = UIColor.black.cgColor
            top.layer.shadowOpacity = 0.25
            top.layer.shadowRadius = 10
            view.addSubview(isBack ? incoming : outgoing)
            view.addSubview(top)
            leaving = outgoing
            coming = incoming
            place(isBack: isBack, pulled: 0, width: width)
        case .changed:
            place(isBack: isBack, pulled: pulled, width: width)
        case .ended, .cancelled, .failed:
            let speed = pan.velocity(in: view).x * (isBack ? 1 : -1)
            let go = pan.state == .ended && (pulled > width * 0.35 || speed > 600) && speed > -200
            UIView.animate(withDuration: 0.3, delay: 0, usingSpringWithDamping: 1, initialSpringVelocity: 0) {
                self.place(isBack: isBack, pulled: go ? width : 0, width: width)
            } completion: { _ in
                if go { self.land(isBack: isBack) } else { self.clear() }
            }
        default:
            break
        }
    }

    /// Where the two snapshots sit with the page pulled this far. The one
    /// underneath moves a third as far, as in a navigation push.
    private func place(isBack: Bool, pulled: CGFloat, width: CGFloat) {
        if isBack {
            leaving?.transform = CGAffineTransform(translationX: pulled, y: 0)
            coming?.transform = CGAffineTransform(translationX: -(width - pulled) / 3, y: 0)
        } else {
            coming?.transform = CGAffineTransform(translationX: width - pulled, y: 0)
            leaving?.transform = CGAffineTransform(translationX: -pulled / 3, y: 0)
        }
    }

    /// Navigate, keeping the snapshot of where we're going on top until the
    /// page has drawn it, then fade it.
    private func land(isBack: Bool) {
        guard let view else { clear(); return }
        leaving?.removeFromSuperview()
        leaving = nil
        coming?.layer.shadowOpacity = 0
        if isBack { view.goBack() } else { view.goForward() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
            UIView.animate(withDuration: 0.15) { self.coming?.alpha = 0 } completion: { _ in self.clear() }
        }
    }

    private func clear() {
        leaving?.removeFromSuperview()
        coming?.removeFromSuperview()
        leaving = nil
        coming = nil
    }

    /// A snapshot on the page's own background: a snapshot of a web page is
    /// see-through in places, and the live page under it would show. With no
    /// snapshot, just the background.
    private func backed(_ shot: UIView?, in view: WKWebView) -> UIView {
        let box = UIView(frame: view.bounds)
        box.backgroundColor = view.underPageBackgroundColor
        box.isUserInteractionEnabled = false
        if let shot {
            shot.frame = box.bounds
            box.addSubview(shot)
        }
        return box
    }
}

/// Calls back as a touch lands, then steps aside: it never recognizes, so it
/// takes nothing from the page.
private final class TouchDown: UIGestureRecognizer {
    private let action: () -> Void
    init(_ action: @escaping () -> Void) {
        self.action = action
        super.init(target: nil, action: nil)
        cancelsTouchesInView = false
        delaysTouchesBegan = false
        delaysTouchesEnded = false
    }
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        action()
        state = .failed
    }
}
#endif
