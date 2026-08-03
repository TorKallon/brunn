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
            app.textFields["login-email"].waitForExistence(timeout: 2),
            "The first-run sign-in screen remained behind startup UI."
        )
        XCTAssertTrue(app.secureTextFields["login-password"].exists)
        XCTAssertTrue(app.buttons["Sign in"].exists)
        XCTAssertFalse(element("straylight-startup", in: app).exists)
    }

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testDemoOpensOnDashboardWithBriefingStorageActivityAndAccess() {
        let app = launchDemo()

        XCTAssertTrue(app.navigationBars["Home"].waitForExistence(timeout: 5))
        XCTAssertTrue(element("dashboard-home", in: app).exists)
        XCTAssertTrue(element("dashboard-storage-text", in: app).exists)
        XCTAssertTrue(element("dashboard-storage-binary", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("Detailed Activity", in: app).exists)
        keepScreenshot(named: "home-dashboard", from: app)

        let operations = element("dashboard-chart-operations", in: app)
        scroll(operations, intoViewIn: app)
        XCTAssertTrue(operations.exists)

        let access = element("dashboard-access-list", in: app)
        scroll(access, intoViewIn: app)
        XCTAssertTrue(access.exists)
        XCTAssertTrue(caseInsensitiveText("This client", in: app).exists)

        app.tabBars.buttons["Home"].tap()
        let briefing = element("dashboard-briefing-action", in: app)
        scroll(briefing, intoViewIn: app)
        briefing.tap()
        XCTAssertTrue(app.navigationBars["Today"].waitForExistence(timeout: 3))
    }

    @MainActor
    func testDemoDashboardLinksReachSearchAndAllBriefings() {
        let app = launchDemo()

        let search = element("dashboard-search", in: app)
        scroll(search, intoViewIn: app)
        search.tap()
        XCTAssertTrue(app.navigationBars["Search"].waitForExistence(timeout: 3))
        app.navigationBars["Search"].buttons.firstMatch.tap()

        let archive = element("dashboard-archive-action", in: app)
        scroll(archive, intoViewIn: app)
        archive.tap()
        XCTAssertTrue(app.navigationBars["Archive"].waitForExistence(timeout: 3))
    }

    @MainActor
    func testDemoSearchOpensPinnedEntryAndTogglesMarkdownFormatting() {
        let app = launchDemo()

        let search = element("dashboard-search", in: app)
        scroll(search, intoViewIn: app)
        search.tap()
        XCTAssertTrue(app.navigationBars["Search"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("search-sort", in: app).exists)

        let field = app.searchFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 2))
        field.tap()
        field.typeText("Straylight")
        app.keyboards.buttons["Search"].tap()

        let result = element("search-result-entry:demo-ios-mvp", in: app)
        XCTAssertTrue(result.waitForExistence(timeout: 3))
        result.tap()

        XCTAssertTrue(app.navigationBars["Entry"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("entry-formatted-content", in: app).waitForExistence(timeout: 2))
        XCTAssertTrue(caseInsensitiveText("Pinned v1", in: app).exists)

        let toggle = element("entry-markdown-toggle", in: app)
        scroll(toggle, intoViewIn: app)
        toggle.tap()
        XCTAssertTrue(element("entry-raw-content", in: app).waitForExistence(timeout: 2))
        XCTAssertTrue(caseInsensitiveText("Raw Markdown", in: app).exists)

        toggle.tap()
        XCTAssertTrue(element("entry-formatted-content", in: app).waitForExistence(timeout: 2))

        let linkedEntry = app.links["Straylight Briefings: Platform Design"]
        XCTAssertTrue(linkedEntry.waitForExistence(timeout: 2))
        scroll(linkedEntry, intoViewIn: app)
        linkedEntry.tap()
        XCTAssertTrue(
            caseInsensitiveText("Straylight Briefings: Platform Design", in: app)
                .waitForExistence(timeout: 3)
        )
    }

    @MainActor
    func testDashboardUsesSingleColumnMetricsAtAccessibilityTextSize() {
        let app = launchDemo(contentSizeCategory: "UICTContentSizeCategoryAccessibilityL")
        let text = element("dashboard-storage-text", in: app)
        let binary = element("dashboard-storage-binary", in: app)

        scroll(text, intoViewIn: app)
        XCTAssertGreaterThan(
            text.frame.width / app.frame.width,
            0.8,
            "Accessibility text sizes should give the text metric a full-width row."
        )
        scroll(binary, intoViewIn: app)
        XCTAssertGreaterThan(
            binary.frame.width / app.frame.width,
            0.8,
            "Accessibility text sizes should give the binary metric a full-width row."
        )
    }

    @MainActor
    func testDemoReaderUsesThePhoneWidthAndShowsTheCompleteSummary() {
        let app = launchDemo()
        openToday(in: app)

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
        openToday(in: app)
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
        openToday(in: app)
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
    func testAlertsOpenDurableDetailBeforeExactBriefingTarget() {
        let app = launchDemo()
        app.tabBars.buttons["Alerts"].tap()

        XCTAssertTrue(app.navigationBars["Alerts"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("alerts-list", in: app).exists)
        XCTAssertTrue(app.buttons["All"].exists)
        XCTAssertTrue(app.buttons["Important"].exists)
        XCTAssertTrue(app.buttons["Unread"].exists)

        app.buttons["Important"].tap()
        let briefingAlert = element("alert-item-11111111111111111111111111111111", in: app)
        XCTAssertTrue(briefingAlert.waitForExistence(timeout: 2))
        XCTAssertEqual(briefingAlert.label, "Open alert")
        scroll(briefingAlert, intoViewIn: app)
        briefingAlert.tap()

        XCTAssertTrue(app.navigationBars["Alert detail"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("alert-detail-11111111111111111111111111111111", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("Your morning briefing is ready", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("Delivery trace", in: app).exists)
        XCTAssertTrue(caseInsensitiveText("Accepted by APNs", in: app).exists)
        XCTAssertTrue(element("alert-target-action", in: app).exists)

        let acknowledge = app.buttons["Acknowledge"]
        scroll(acknowledge, intoViewIn: app)
        acknowledge.tap()
        XCTAssertTrue(app.buttons["Acknowledged"].waitForExistence(timeout: 2))
        XCTAssertFalse(app.buttons["Acknowledged"].isEnabled)

        let target = element("alert-target-action", in: app)
        scroll(target, intoViewIn: app)
        target.tap()
        XCTAssertTrue(app.navigationBars["Today"].waitForExistence(timeout: 3))
        XCTAssertTrue(element("briefing-item-ios-direction", in: app).exists)

        keepScreenshot(named: "alert-to-exact-briefing-item", from: app)
    }

    @MainActor
    func testSettingsHidesLegacyTopicsAndPersistsAppearance() {
        let app = launchDemo()
        app.tabBars.buttons["Settings"].tap()

        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 3))
        XCTAssertFalse(app.staticTexts["Tracked topics"].exists)
        XCTAssertFalse(app.staticTexts["Pending deep-dives"].exists)

        let appearance = app.segmentedControls["appearance-mode"]
        XCTAssertTrue(appearance.waitForExistence(timeout: 2))
        let light = appearance.buttons["Light"]
        light.tap()
        XCTAssertTrue(light.isSelected)

        app.terminate()
        app.launch()
        XCTAssertTrue(app.tabBars.buttons["Settings"].waitForExistence(timeout: 5))
        app.tabBars.buttons["Settings"].tap()

        let restoredAppearance = app.segmentedControls["appearance-mode"]
        XCTAssertTrue(restoredAppearance.waitForExistence(timeout: 2))
        XCTAssertTrue(restoredAppearance.buttons["Light"].isSelected)
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
    private func launchDemo(
        contentSizeCategory: String = "UICTContentSizeCategoryL"
    ) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--demo",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
            "-UIPreferredContentSizeCategoryName", contentSizeCategory,
        ]
        app.launchEnvironment["TZ"] = "America/Los_Angeles"
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 5))
        return app
    }

    @MainActor
    private func openToday(in app: XCUIApplication) {
        let today = app.tabBars.buttons["Today"]
        XCTAssertTrue(today.waitForExistence(timeout: 3))
        today.tap()
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
