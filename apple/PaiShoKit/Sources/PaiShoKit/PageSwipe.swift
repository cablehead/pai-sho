#if os(iOS)
import UIKit
import UIKit.UIGestureRecognizerSubclass
import WebKit

/// Sideways swipes on a peer's page, from anywhere on it, that slide it under
/// the finger. Pull far enough, or flick, and it carries on to the next page;
/// let go early and it falls back. Swiping right shows the page you came from
/// underneath, from a snapshot taken while you were on it. With no history
/// left, the navigation's own swipe slides the page away to the main screen.
/// A tap that opens a new page slides it in from the right, as a push does.
///
/// WebKit's own swipe (`allowsBackForwardNavigationGestures`) never began
/// here, inside SwiftUI's navigation, so the slide is done by hand.
@MainActor
final class PageSwipe: NSObject, UIGestureRecognizerDelegate {
    private weak var view: WKWebView?
    private let drag = UIPanGestureRecognizer()
    /// The way the current slide goes: true for back.
    private var slidingBack = true
    private weak var navigation: UINavigationController?
    private var watches: [NSKeyValueObservation] = []
    /// How each page in the history looked when last seen.
    private var looks: [WKBackForwardListItem: UIView] = [:]
    /// The snapshots on screen during a slide: the page being left, and the
    /// one coming in.
    private var leaving: UIView?
    private var coming: UIView?
    /// Where the history stood, to tell a page opened by a tap (a push) from
    /// one reached by a swipe.
    private var lastItem: WKBackForwardListItem?
    private var lastDepth = 0
    private var swiping = false

    init(_ view: WKWebView) {
        self.view = view
        super.init()
        drag.addTarget(self, action: #selector(slide(_:)))
        drag.delegate = self
        view.addGestureRecognizer(drag)
        // The page's scrolling waits until the drag turns out not sideways.
        view.scrollView.panGestureRecognizer.require(toFail: drag)
        // A look at the page as a touch lands, before a tap navigates away.
        view.addGestureRecognizer(TouchDown { [weak self] in self?.remember() })
        watches = [
            view.observe(\.canGoBack) { [weak self] _, _ in MainActor.assumeIsolated { self?.gate() } },
            view.observe(\.url) { [weak self] _, _ in MainActor.assumeIsolated { self?.moved() } },
        ]
        gate()
    }

    /// Find the navigation once the page is on screen.
    func attach() {
        guard let view, navigation == nil else { gate(); return }
        let parent = sequence(first: view.next, next: { $0?.next })
            .first { $0 is UINavigationController } as? UINavigationController
        guard let parent else { return }
        navigation = parent
        // SwiftUI turns the navigation's swipes off with its bar hidden.
        for pop in pops {
            pop.delegate = self
            view.scrollView.panGestureRecognizer.require(toFail: pop)
        }
        gate()
    }

    /// The navigation's own swipes back: from the edge, and (iOS 26) from
    /// anywhere on the screen.
    private var pops: [UIGestureRecognizer] {
        guard let navigation else { return [] }
        var pops = [navigation.interactivePopGestureRecognizer].compactMap { $0 }
        if #available(iOS 26, *), let content = navigation.interactiveContentPopGestureRecognizer {
            pops.append(content)
        }
        return pops
    }

    /// Back slides within the page while it has history, and the navigation's
    /// swipes take over once it has none.
    private func gate() {
        guard let view else { return }
        for pop in pops { pop.isEnabled = !view.canGoBack }
    }

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard gesture === drag else {
            return (navigation?.viewControllers.count ?? 0) > 1
        }
        guard let view else { return false }
        let speed = drag.velocity(in: view)
        // Sideways, not a scroll down the page.
        guard abs(speed.x) > abs(speed.y) * 1.5 else { return false }
        let back = speed.x > 0
        guard back ? view.canGoBack : view.canGoForward else { return false }
        // A page wider than the screen scrolls sideways first.
        let scroll = view.scrollView
        let room = scroll.contentSize.width - scroll.bounds.width
        if room > 1 {
            let x = scroll.contentOffset.x + scroll.adjustedContentInset.left
            if back ? x > 1 : x < room - 1 { return false }
        }
        return true
    }

    private func remember() {
        guard let view, let item = view.backForwardList.currentItem,
              let look = look(of: view) else { return }
        looks[item] = look
        // Where the history stands as a touch lands: a tap that opens a page
        // is measured from here (the first page's entry is not there yet
        // when it starts loading).
        if !swiping, leaving == nil {
            lastItem = item
            lastDepth = view.backForwardList.backList.count
        }
        let kept = Set(view.backForwardList.backList + view.backForwardList.forwardList + [item])
        looks = looks.filter { kept.contains($0.key) }
    }

    @objc private func slide(_ pan: UIPanGestureRecognizer) {
        guard let view else { return }
        if pan.state == .began { slidingBack = pan.velocity(in: view).x > 0 }
        let isBack = slidingBack
        let width = view.bounds.width
        let pulled = min(max(isBack ? pan.translation(in: view).x : -pan.translation(in: view).x, 0), width)
        switch pan.state {
        case .began:
            remember()
            let item = isBack ? view.backForwardList.backItem : view.backForwardList.forwardItem
            let incoming = backed(item.flatMap { looks[$0] }, in: view)
            let outgoing = backed(look(of: view), in: view)
            // Back: the page slides off to the right, the one before it
            // waiting underneath. Forward: the next one slides in over it.
            let top = isBack ? outgoing : incoming
            shade(top)
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

    /// The page moved in its history. One a tap opened slides in from the
    /// right over the page it came from, as a push does.
    private func moved() {
        guard let view else { return }
        // Cover with how the page looked, before the new one shows. Whether it
        // was a push is only known a moment later: the history updates after
        // the address does.
        var cover: UIView?
        if !swiping, leaving == nil, coming == nil, let last = lastItem, let before = looks[last] {
            let shot = backed(before, in: view)
            view.addSubview(shot)
            leaving = shot
            cover = shot
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
            let item = view.backForwardList.currentItem
            let depth = view.backForwardList.backList.count
            let pushed = item !== self.lastItem && depth > self.lastDepth
            self.lastItem = item
            self.lastDepth = depth
            guard let cover, self.leaving === cover else { return }
            guard pushed else { self.clear(); return }
            let width = view.bounds.width
            let incoming = self.backed(self.look(of: view), in: view)
            self.shade(incoming)
            view.addSubview(incoming)
            self.coming = incoming
            self.place(isBack: false, pulled: 0, width: width)
            UIView.animate(withDuration: 0.35, delay: 0, usingSpringWithDamping: 1, initialSpringVelocity: 0) {
                self.place(isBack: false, pulled: width, width: width)
            } completion: { _ in self.clear() }
        }
    }

    /// The page as it is on screen: its scroll view, without any snapshot
    /// laid over it.
    private func look(of view: WKWebView) -> UIView? {
        view.scrollView.snapshotView(afterScreenUpdates: false)
    }

    private func shade(_ top: UIView) {
        top.layer.shadowColor = UIColor.black.cgColor
        top.layer.shadowOpacity = 0.25
        top.layer.shadowRadius = 10
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
        swiping = true
        if isBack { view.goBack() } else { view.goForward() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
            self.swiping = false
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
