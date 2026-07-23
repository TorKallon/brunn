import XCTest

@MainActor
final class RuptureOpsUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testMissionTimelineReferenceState() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-test-reference"]
        app.launch()

        XCTAssertTrue(app.staticTexts["QUIET WINDOW"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["07:42"].exists)
        XCTAssertTrue(app.staticTexts["Lower ambient risk"].exists)
        XCTAssertTrue(app.staticTexts["Finish or leave hard POIs"].exists)
        XCTAssertTrue(app.staticTexts["Retreat warning"].exists)
        XCTAssertTrue(app.staticTexts["Vermin returning"].exists)
        XCTAssertTrue(app.buttons["mute-one-cycle"].isHittable)
        XCTAssertTrue(app.buttons["resync"].isHittable)
    }

    func testMuteAndPauseControlsWork() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-test-reference"]
        app.launch()

        let muteButton = app.buttons["mute-one-cycle"]
        XCTAssertTrue(muteButton.waitForExistence(timeout: 5))
        muteButton.tap()
        XCTAssertTrue(app.staticTexts["Muted until next Ruptura"].waitForExistence(timeout: 2))

        app.buttons["resync"].tap()
        XCTAssertTrue(app.navigationBars["Session sync"].waitForExistence(timeout: 2))

        let pauseButton = app.buttons["pause-resume"]
        XCTAssertTrue(pauseButton.exists)
        pauseButton.tap()
        XCTAssertTrue(app.buttons["Resume timer"].waitForExistence(timeout: 2))
    }

    func testUnsyncedSessionCanBeAnchored() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-testing", "--ui-test-unsynced"]
        app.launch()

        XCTAssertTrue(app.staticTexts["UNSYNCED"].waitForExistence(timeout: 5))
        app.buttons["open-sync"].tap()
        XCTAssertTrue(app.navigationBars["Session sync"].waitForExistence(timeout: 2))
        app.buttons["sync-fire-wave-now"].tap()
        XCTAssertTrue(app.staticTexts["FIRE WAVE"].waitForExistence(timeout: 2))
    }
}
