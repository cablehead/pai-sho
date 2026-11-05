import XCTest

/// Drives the app in the simulator against a real pai-sho peer on this Mac.
/// The invitation comes from the PAI_SHO_INVITE environment variable, and the
/// port the peer exposes from PAI_SHO_PORT; xcodebuild passes both through as
/// TEST_RUNNER_PAI_SHO_INVITE and TEST_RUNNER_PAI_SHO_PORT. Skipped without
/// them.
final class PaiShoUITests: XCTestCase {
    func testPasteAcceptAndOpenAPort() throws {
        let env = ProcessInfo.processInfo.environment
        guard let invite = env["PAI_SHO_INVITE"], let port = env["PAI_SHO_PORT"] else {
            throw XCTSkip("set TEST_RUNNER_PAI_SHO_INVITE and TEST_RUNNER_PAI_SHO_PORT")
        }
        let expect = env["PAI_SHO_EXPECT"] ?? ""

        UIPasteboard.general.string = invite
        var app = XCUIApplication()
        app.launch()

        let paste = app.buttons["Paste"]
        XCTAssertTrue(paste.waitForExistence(timeout: 10))
        attach(app, "before-paste")
        paste.tap()

        var name = app.textFields["name"]
        if !name.waitForExistence(timeout: 10) {
            // The system Paste button may refuse a synthesized tap. Open the
            // same sheet with the invitation the way the app's -invite launch
            // argument does, and go on from there.
            attach(app, "paste-did-not-open")
            app.terminate()
            app = XCUIApplication()
            app.launchArguments = ["-invite", invite]
            app.launch()
            name = app.textFields["name"]
            XCTAssertTrue(name.waitForExistence(timeout: 10))
        }
        name.tap()
        name.typeText("mac")
        attach(app, "accept-sheet")
        app.buttons["Accept"].tap()

        let row = app.buttons["port-\(port)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "port \(port) never listed")
        let bound = NSPredicate(format: "label CONTAINS '127.0.0.1:'")
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

    /// Stop and Start the process, and open Scan, which in the simulator has
    /// no camera and says so.
    func testStopStartAndScan() {
        let app = XCUIApplication()
        app.launch()

        XCTAssertTrue(app.staticTexts["Running"].waitForExistence(timeout: 10))
        app.buttons["Stop"].tap()
        XCTAssertTrue(app.staticTexts["Stopped"].waitForExistence(timeout: 10))
        attach(app, "stopped")
        app.buttons["Start"].tap()
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
