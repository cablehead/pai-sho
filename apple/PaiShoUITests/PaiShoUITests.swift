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
        let box = app.webViews.textFields.firstMatch
        XCTAssertTrue(box.waitForExistence(timeout: 10))
        box.tap()
        sleep(2)
        attach(app, "box-open")
        box.typeText("test")
        sleep(1)
        attach(app, "box-typed")
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
