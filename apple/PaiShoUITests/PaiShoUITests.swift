import XCTest

/// Drives the app in the simulator against a real pai-sho peer on this Mac.
/// The invitation comes from the PAI_SHO_INVITE environment variable, and the
/// port the peer exposes from PAI_SHO_PORT; xcodebuild passes both through as
/// TEST_RUNNER_PAI_SHO_INVITE and TEST_RUNNER_PAI_SHO_PORT. Skipped without
/// them.
final class PaiShoUITests: XCTestCase {
    func testAcceptAndOpenAPort() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"] else {
            throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE and TEST_RUNNER_PAI_SHO_PORT")
        }
        let expect = env["PAI_SHO_EXPECT"] ?? ""

        // The phone takes an invitation by scanning it, which a simulator
        // cannot do, so the test hands it over with the -invite launch
        // argument. That opens the same sheet a scan does.
        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        XCTAssertTrue(name.waitForExistence(timeout: 10))
        name.tap()
        name.typeText("mac")
        attach(app, "accept-sheet")
        app.buttons["Accept"].tap()

        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        let bound = NSPredicate(format: "isHittable == true")
        expectation(for: bound, evaluatedWith: row)
        waitForExpectations(timeout: 60)
        attach(app, "peers")
        row.tap()

        if !expect.isEmpty {
            let text = app.webViews.staticTexts[expect]
            XCTAssertTrue(text.waitForExistence(timeout: 30), "page never showed \(expect)")
        }
        attach(app, "page")
    }

    /// The App Store screenshots: the add sheet, the peer with its ports, and
    /// one port's page. PAI_SHO_SHOTS_NAME names the peer; PAI_SHO_PORT is
    /// the port whose page is shown. Skipped without PAI_SHO_SHOTS_NAME.
    func testStoreScreenshots() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let peer = env["PAI_SHO_SHOTS_NAME"], let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_SHOTS_NAME, _INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        XCTAssertTrue(name.waitForExistence(timeout: 10))
        name.tap()
        // A fresh simulator's keyboard shows a one-time typing tip over the sheet.
        let tip = app.buttons["Continue"]
        sleep(1)
        if tip.exists { tip.tap() }
        name.typeText(peer)
        attach(app, "1-add")
        app.buttons["Accept"].tap()

        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        expectation(for: NSPredicate(format: "isHittable == true"), evaluatedWith: row)
        waitForExpectations(timeout: 60)
        sleep(2)
        attach(app, "2-peers")
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30), "page never showed \(expect)")
        sleep(2)
        attach(app, "3-page")
    }

    /// An invitation that expired leaves its peer stuck connecting. A fresh
    /// one from the same machine still goes through, and a long press on a
    /// peer forgets it.
    func testExpiredInvitation() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let key = invite.split(separator: ".").first
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE and _PORT") }
        let stale = "\(key).\(String(repeating: "00", count: 16))"

        func take(_ invitation: String, as peer: String) -> XCUIApplication {
            let app = XCUIApplication()
            app.launchArguments = ["-invite", invitation]
            app.launch()
            let name = app.textFields["name"]
            XCTAssertTrue(name.waitForExistence(timeout: 10))
            name.tap()
            sleep(1)
            if app.buttons["Continue"].exists { app.buttons["Continue"].tap() }
            name.typeText(peer)
            app.buttons["Accept"].tap()
            return app
        }

        var app = take(stale, as: "old")
        let old = app.descendants(matching: .any)["peer-old"].firstMatch
        XCTAssertTrue(old.waitForExistence(timeout: 10))
        sleep(4)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'connecting'")).firstMatch.exists)
        attach(app, "1-stuck")

        app.terminate()
        app = take(invite, as: "demo")
        XCTAssertTrue(app.buttons["port-\(port)"].waitForExistence(timeout: 60), "a fresh invitation never got through")
        sleep(2)
        attach(app, "2-fresh")

        app.buttons["port-\(port)"].press(forDuration: 1.5)
        let forget = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Forget '")).firstMatch
        XCTAssertTrue(forget.waitForExistence(timeout: 5))
        attach(app, "3-menu")
        forget.tap()
        let confirm = app.buttons["Forget"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5))
        attach(app, "4-confirm")
        confirm.tap()
        XCTAssertTrue(app.staticTexts["No peers"].waitForExistence(timeout: 10))
        attach(app, "5-gone")
    }

    /// The credits under the peer list, and the licences they open.
    func testAcknowledgements() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE and _PORT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            sleep(1)
            if app.buttons["Continue"].exists { app.buttons["Continue"].tap() }
            name.typeText("demo")
            app.buttons["Accept"].tap()
        }
        XCTAssertTrue(app.buttons["port-\(port)"].waitForExistence(timeout: 60), "port \(port) never listed")
        let credits = app.buttons["acknowledgements"]
        XCTAssertTrue(credits.waitForExistence(timeout: 10))
        sleep(2)
        attach(app, "credits")
        credits.tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'number 0'")).firstMatch.waitForExistence(timeout: 10))
        sleep(1)
        attach(app, "acknowledgements")
    }

    /// A browser's controls under the page: back and forward through its
    /// history, and a button out to the main screen from anywhere in it.
    func testPageControls() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            name.typeText("mac")
            app.buttons["Accept"].tap()
        }
        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        row.tap()
        let first = app.webViews.staticTexts[expect]
        XCTAssertTrue(first.waitForExistence(timeout: 30), "page never showed \(expect)")
        let back = app.buttons["back"], forward = app.buttons["forward"]
        XCTAssertFalse(back.isEnabled, "back with no history")

        // Somewhere new in the page, then back and forward again.
        app.webViews.links.matching(NSPredicate(format: "label BEGINSWITH '#'")).firstMatch.tap()
        XCTAssertTrue(app.webViews.staticTexts["page two"].waitForExistence(timeout: 10))
        attach(app, "page-two")
        XCTAssertTrue(back.isEnabled)
        back.tap()
        XCTAssertTrue(first.waitForExistence(timeout: 10), "back did not go back")
        XCTAssertTrue(forward.isEnabled)
        forward.tap()
        XCTAssertTrue(app.webViews.staticTexts["page two"].waitForExistence(timeout: 10), "forward did not go forward")

        // Out to the main screen, with history still behind the page.
        app.buttons["peers"].tap()
        XCTAssertTrue(row.waitForExistence(timeout: 10), "did not reach the main screen")
        attach(app, "main-screen")

        // In again: the page is where it was left, as a browser's tab is.
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts["page two"].waitForExistence(timeout: 10), "the page started over")
        XCTAssertTrue(back.isEnabled, "the page's history was lost")
    }

    /// Out to the main screen from another page of the site and back in:
    /// that page, not the port's front page.
    func testKeepsTheTabAcrossPages() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            name.typeText("mac")
            app.buttons["Accept"].tap()
        }
        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30))
        let link = env["PAI_SHO_LINK"] ?? "other page", there = env["PAI_SHO_THERE"] ?? "the other page"
        let contents = app.webViews.staticTexts["contents"]
        if !app.webViews.links[link].isHittable, contents.exists { contents.tap() }
        app.webViews.links[link].firstMatch.tap()
        XCTAssertTrue(app.webViews.staticTexts[there].waitForExistence(timeout: 10))
        attach(app, "there")

        app.buttons["peers"].tap()
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        attach(app, "home")
        row.tap()
        sleep(2)
        attach(app, "back-in")
        XCTAssertTrue(app.webViews.staticTexts[there].exists, "back at the front page")
    }

    /// Back from the background, the page stays on screen while pai-sho
    /// restarts and its port comes back, rather than a "not reachable"
    /// screen in its place.
    func testReturnKeepsThePage() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            name.typeText("mac")
            app.buttons["Accept"].tap()
        }
        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30))

        XCUIDevice.shared.press(.home)
        sleep(4)
        app.activate()
        attach(app, "back")
        XCTAssertFalse(app.staticTexts["Not reachable yet"].exists, "the page was replaced")
        XCTAssertTrue(app.webViews.firstMatch.exists, "the page is gone")
        sleep(1)
        attach(app, "back-1s")
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30))
        attach(app, "back-later")
    }

    /// A link that opens a new tab (n2's card links do) opens the peer's
    /// page in place, rather than nothing happening.
    func testNewTabLinkOpensHere() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            name.typeText("mac")
            app.buttons["Accept"].tap()
        }
        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30))
        app.webViews.links["new tab"].tap()
        XCTAssertTrue(app.webViews.staticTexts["opened in place"].waitForExistence(timeout: 10), "the link did nothing")
        attach(app, "opened")
    }

    /// Switch the process off and on, and open Scan, which in the simulator has
    /// no camera and says so.
    func testStopStartAndScan() {
        let app = XCUIApplication()
        app.launch()

        XCTAssertTrue(app.staticTexts["Running"].waitForExistence(timeout: 10))
        attach(app, "running")
        // The switch itself sits at the right of its row.
        let toggle = app.switches["running"].coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5))
        toggle.tap()
        XCTAssertTrue(app.staticTexts["Stopped"].waitForExistence(timeout: 10))
        attach(app, "stopped")
        toggle.tap()
        XCTAssertTrue(app.staticTexts["Running"].waitForExistence(timeout: 10))

        app.buttons["Scan"].tap()
        XCTAssertTrue(app.navigationBars["Scan"].waitForExistence(timeout: 10))
        let none = app.staticTexts["No camera"]
        if none.waitForExistence(timeout: 5) {
            attach(app, "scan-no-camera")
        } else {
            attach(app, "scan")
        }
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["Scan"].waitForExistence(timeout: 10))
    }

    /// A page's own text box near the top, opened: it stays on screen with
    /// the keyboard up. And the end of a long page scrolls clear of the
    /// buttons under it.
    func testPageEdges() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"],
              let expect = env["PAI_SHO_EXPECT"]
        else { throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE, _PORT and _EXPECT") }

        let app = XCUIApplication()
        app.launchArguments = ["-invite", invite]
        app.launch()
        let name = app.textFields["name"]
        if name.waitForExistence(timeout: 10) {
            name.tap()
            name.typeText("mac")
            app.buttons["Accept"].tap()
        }
        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        row.tap()
        XCTAssertTrue(app.webViews.staticTexts[expect].waitForExistence(timeout: 30))
        sleep(1)
        attach(app, "page")
        // The start page itself, scrolled all the way down.
        for _ in 0..<25 { app.webViews.firstMatch.swipeUp(velocity: .fast) }
        sleep(2)
        attach(app, "page-bottom")
        for _ in 0..<25 { app.webViews.firstMatch.swipeDown(velocity: .fast) }
        sleep(1)
        // The same, with no typing first.
        if let tap = env["PAI_SHO_TAP_FIRST"], !tap.isEmpty {
            openAndScroll(app, tap, twice: true)
            return
        }
        let box = app.webViews.textFields.firstMatch
        XCTAssertTrue(box.waitForExistence(timeout: 10))
        box.tap()
        sleep(2)
        attach(app, "box-open")
        box.typeText("test")
        sleep(1)
        attach(app, "box-typed")
        box.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 4))
        attach(app, "box-cleared")
        let done = app.toolbars.buttons["Done"]
        if done.exists { done.tap() } else { app.webViews.staticTexts[expect].firstMatch.tap() }
        sleep(1)

        // A short page about one screen tall, opened after typing, scrolled
        // down: the keyboard going away must not take the page's end back
        // under the buttons.
        if let tap = env["PAI_SHO_TAP"], !tap.isEmpty {
            openAndScroll(app, tap, twice: true)
            return
        }

        // The end of a long page, scrolled all the way down: clear of the
        // buttons, with the page showing through the strip behind them.
        guard let long = env["PAI_SHO_LONG"], !long.isEmpty else { return }
        // The page's contents fold, at the left of its bar.
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: 31, dy: 87)).tap()
        sleep(1)
        attach(app, "contents")
        app.webViews.links[long].firstMatch.tap()
        sleep(3)
        // Reading down: the buttons step aside; a little back up, they return.
        for _ in 0..<3 { app.webViews.firstMatch.swipeUp() }
        sleep(1)
        attach(app, "reading")
        app.webViews.firstMatch.swipeDown()
        sleep(1)
        attach(app, "back-up")
        for _ in 0..<25 { app.webViews.firstMatch.swipeUp(velocity: .fast) }
        sleep(2)
        attach(app, "bottom")
    }

    /// Opens the thread whose card shows `text`, scrolls to its end, and
    /// attaches that. After the keyboard, the first tap only picks the card.
    private func openAndScroll(_ app: XCUIApplication, _ text: String, twice: Bool) {
        let card = app.webViews.firstMatch.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", text)).element(boundBy: 0)
        card.tap()
        sleep(1)
        if twice, card.exists, card.isHittable {
            card.tap()
            sleep(2)
        }
        attach(app, "tapped")
        // A thread opens at its newest card, its end: back up, then read down.
        for _ in 0..<4 { app.webViews.firstMatch.swipeDown() }
        app.webViews.firstMatch.swipeUp()
        sleep(1)
        attach(app, "tapped-reading")
        for _ in 0..<25 { app.webViews.firstMatch.swipeUp(velocity: .fast) }
        sleep(2)
        attach(app, "tapped-bottom")
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
