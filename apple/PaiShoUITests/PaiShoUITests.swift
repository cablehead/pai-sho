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

    /// On a phone the page has no header, so the swipes are the way around:
    /// right goes back in the page (an n2 thread to its list), and with no
    /// history left, right leaves for the main screen. Needs an n2 board on
    /// PAI_SHO_PORT, and PAI_SHO_EXPECT set to text on its list.
    func testSwipeBackThenOut() throws {
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

        let list = app.webViews.staticTexts[expect]
        XCTAssertTrue(list.waitForExistence(timeout: 30), "board never showed \(expect)")
        attach(app, "board-list")

        // Into the first task on the list.
        let task = app.webViews.links.matching(NSPredicate(format: "label BEGINSWITH '#'")).firstMatch
        if task.waitForExistence(timeout: 5) { task.tap() } else {
            app.webViews.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45)).tap()
        }
        sleep(2)
        attach(app, "board-thread")

        // Pulled partway within the page and let go: it stays where it was.
        let before = app.webViews.firstMatch.frame
        nudgeFromLeftEdge(app, to: 0.3, hold: 2)
        sleep(1)
        attach(app, "after-page-nudge")
        // A live page changes by itself (n2's timer), so compare where it
        // sits, not its pixels.
        XCTAssertEqual(app.webViews.firstMatch.frame, before, "a short pull moved the page")
        XCTAssertFalse(row.isHittable, "a short pull left the page")

        // From the middle of the screen, not the edge.
        swipeFromLeftEdge(app, from: 0.3)
        sleep(2)
        attach(app, "after-first-swipe")
        XCTAssertTrue(app.webViews.firstMatch.exists, "first swipe left the page")
        XCTAssertTrue(list.waitForExistence(timeout: 5), "first swipe did not go back to \(expect)")

        // Pulled a little and let go: the page falls back where it was.
        nudgeFromLeftEdge(app, to: 0.3, hold: 2)
        sleep(1)
        attach(app, "after-nudge")
        XCTAssertFalse(row.isHittable, "a short pull left the page")

        swipeFromLeftEdge(app, from: 0.2, to: 0.95)
        XCTAssertTrue(row.waitForExistence(timeout: 10), "second swipe did not reach the main screen")
        attach(app, "main-screen")
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

    private func nudgeFromLeftEdge(_ app: XCUIApplication, to x: CGFloat = 0.15, hold: TimeInterval = 0.3) {
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: x, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: hold)
    }

    private func swipeFromLeftEdge(_ app: XCUIApplication, from x: CGFloat = 0.01, to endX: CGFloat = 0.8) {
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: x, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: endX, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: end)
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

    private func attach(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
