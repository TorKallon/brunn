import XCTest

final class StraylightUITests: XCTestCase {
    @MainActor
    func testFirstRunConnectionScreenAppearsPromptly() {
        let app = XCUIApplication()
        app.launchArguments = [
            "--ui-test-connection-required",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
        ]

        app.launch()

        XCTAssertTrue(
            app.buttons["Connect this iPhone"].waitForExistence(timeout: 2),
            "The first-run connection screen remained behind startup UI."
        )
        XCTAssertFalse(element("straylight-startup", in: app).exists)
    }

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testDemoReaderUsesThePhoneWidthAndShowsTheCompleteSummary() {
        let app = launchDemo()

        XCTAssertTrue(app.navigationBars["Today"].waitForExistence(timeout: 5))

        let reader = element("briefing-reader", in: app)
        let title = element("briefing-reader-title", in: app)
        let summary = element("briefing-summary", in: app)
        XCTAssertTrue(reader.waitForExistence(timeout: 3))
        XCTAssertTrue(title.exists)
        XCTAssertTrue(summary.exists)

        let horizontalInset = summary.frame.minX - app.frame.minX
        XCTAssertLessThanOrEqual(
            horizontalInset,
            20,
            "The briefing summary should not retain a timeline gutter on compact screens."
        )
        XCTAssertGreaterThanOrEqual(
            summary.frame.width / app.frame.width,
            0.90,
            "The primary reading card should use at least 90% of the phone width."
        )

        for index in 0 ..< 7 {
            XCTAssertTrue(
                element("briefing-summary-line-\(index)", in: app).exists,
                "Demo mode must expose every deterministic summary line by default."
            )
        }

        keepScreenshot(named: "briefing-reader-complete-summary", from: app)
    }

    @MainActor
    func testSummaryCanCollapseAndRestoreWithoutLosingPriorityItems() {
        let app = launchDemo()
        let toggle = element("briefing-summary-toggle", in: app)
        XCTAssertTrue(toggle.waitForExistence(timeout: 5))

        scroll(toggle, intoViewIn: app)
        XCTAssertEqual(toggle.label, "Show fewer summary items")
        toggle.tap()

        XCTAssertTrue(
            element("briefing-summary-line-4", in: app).waitForNonExistence(timeout: 2)
        )
        XCTAssertEqual(toggle.label, "Show all 7 summary items")
        for index in 0 ..< 3 {
            XCTAssertTrue(element("briefing-summary-line-\(index)", in: app).exists)
        }

        toggle.tap()
        XCTAssertTrue(element("briefing-summary-line-4", in: app).waitForExistence(timeout: 2))
        XCTAssertEqual(toggle.label, "Show fewer summary items")
    }

    @MainActor
    func testEveryDemoSectionAndItemDetailIsReachableWithSourcesAndHistory() {
        let app = launchDemo()
        XCTAssertTrue(element("briefing-reader", in: app).waitForExistence(timeout: 5))

        XCTAssertTrue(element("briefing-section-straylight", in: app).exists)
        XCTAssertTrue(element("briefing-section-platform", in: app).exists)
        XCTAssertTrue(element("briefing-section-reading-experience", in: app).exists)
        XCTAssertTrue(element("briefing-item-ios-direction", in: app).exists)
        XCTAssertTrue(element("briefing-item-existing-contracts", in: app).exists)
        XCTAssertTrue(element("briefing-item-delivery-correction", in: app).exists)
        XCTAssertTrue(element("briefing-item-full-width-reader", in: app).exists)

        let firstItem = element("briefing-item-ios-direction", in: app)
        scroll(firstItem, intoViewIn: app)
        firstItem.tap()

        let detail = element("briefing-item-detail-ios-direction", in: app)
        XCTAssertTrue(detail.waitForExistence(timeout: 2))
        scroll(detail, intoViewIn: app)
        XCTAssertTrue(app.staticTexts["What changed"].exists)
        XCTAssertTrue(app.staticTexts["Why it matters"].exists)
        XCTAssertTrue(app.staticTexts["SOURCES"].exists)

        let history = element("briefing-revision-history", in: app)
        scroll(history, intoViewIn: app)
        history.tap()

        let currentVersion = element("briefing-version-2", in: app)
        XCTAssertTrue(currentVersion.waitForExistence(timeout: 2))
        XCTAssertTrue(element("briefing-version-1", in: app).exists)
        XCTAssertTrue(currentVersion.label.localizedCaseInsensitiveContains("current"))
    }

    @MainActor
    func testNewsFiltersOpenTheSourceBackedDetailAndTrackSessionReadState() {
        let app = launchDemo()
        app.tabBars.buttons["News"].tap()

        XCTAssertTrue(app.navigationBars["News"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("news-list", in: app).exists)
        XCTAssertTrue(app.buttons["All"].exists)
        XCTAssertTrue(app.buttons["Priority"].exists)
        XCTAssertTrue(app.buttons["Unread"].exists)

        app.buttons["Priority"].tap()
        let update = element("news-item-ios-direction", in: app)
        XCTAssertTrue(update.waitForExistence(timeout: 2))
        XCTAssertEqual(update.label, "Open update")
        scroll(update, intoViewIn: app)
        update.tap()

        XCTAssertTrue(app.navigationBars["News detail"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("news-detail-ios-direction", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("Why it matters", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("What changed", in: app).exists)
        XCTAssertTrue(app.staticTexts["Sources"].exists)

        let markRead = app.buttons["Mark as read"]
        scroll(markRead, intoViewIn: app)
        markRead.tap()
        XCTAssertTrue(app.buttons["Read"].waitForExistence(timeout: 2))
        XCTAssertFalse(app.buttons["Read"].isEnabled)

        app.navigationBars["News detail"].buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["News"].waitForExistence(timeout: 2))
        app.buttons["Unread"].tap()
        XCTAssertTrue(update.waitForNonExistence(timeout: 2))
        XCTAssertTrue(element("news-item-delivery-correction", in: app).exists)

        keepScreenshot(named: "news-unread-after-session-read", from: app)
    }

    @MainActor
    func testArchiveOpensPriorEditionsAndPinnedRevision() {
        let app = launchDemo()
        app.tabBars.buttons["Archive"].tap()

        XCTAssertTrue(app.navigationBars["Archive"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("briefing-archive-list", in: app).exists)
        XCTAssertTrue(element("briefing-archive-2026-08-02-morning", in: app).exists)
        XCTAssertTrue(element("briefing-archive-2026-08-01-evening", in: app).exists)
        XCTAssertTrue(element("briefing-archive-2026-08-01-morning", in: app).exists)

        let currentEdition = element("briefing-archive-2026-08-02-morning", in: app)
        scroll(currentEdition, intoViewIn: app)
        currentEdition.tap()

        let versionSelector = element("briefing-version-selector", in: app)
        XCTAssertTrue(versionSelector.waitForExistence(timeout: 3))
        versionSelector.tap()
        let versionOne = app.buttons["Version 1"]
        XCTAssertTrue(versionOne.waitForExistence(timeout: 2))
        versionOne.tap()

        let pinnedVersion = NSPredicate { _, _ in
            String(describing: versionSelector.value).localizedCaseInsensitiveContains("Version 1")
        }
        expectation(for: pinnedVersion, evaluatedWith: nil)
        waitForExpectations(timeout: 3)
        XCTAssertTrue(element("briefing-reader", in: app).exists)
    }

    @MainActor
    private func launchDemo() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--demo",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryL",
        ]
        app.launchEnvironment["TZ"] = "America/Los_Angeles"
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 5))
        return app
    }

    @MainActor
    private func element(_ identifier: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    @MainActor
    private func caseInsensitiveText(_ label: String, in app: XCUIApplication) -> XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label ==[c] %@", label)).firstMatch
    }

    @MainActor
    private func scroll(
        _ element: XCUIElement,
        intoViewIn app: XCUIApplication,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        for _ in 0 ..< 12 where !element.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(element.isHittable, "Element never became hittable.", file: file, line: line)
    }

    @MainActor
    private func keepScreenshot(named name: String, from app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
