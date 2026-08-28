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
    func testDemoOpensOnDashboardWithSearchStorageActivityAndAccess() {
        let app = launchDemo()

        XCTAssertTrue(app.navigationBars["Home"].waitForExistence(timeout: 5))
        XCTAssertTrue(element("dashboard-home", in: app).exists)
        XCTAssertTrue(element("dashboard-search", in: app).exists)
        XCTAssertFalse(app.staticTexts["Your Straylight"].exists)
        XCTAssertFalse(element("dashboard-briefing-action", in: app).exists)
        XCTAssertFalse(element("dashboard-archive-action", in: app).exists)
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
    }

    @MainActor
    func testDemoDashboardLinkReachesSearch() {
        let app = launchDemo()

        let search = element("dashboard-search", in: app)
        scroll(search, intoViewIn: app)
        search.tap()
        XCTAssertTrue(app.navigationBars["Search"].waitForExistence(timeout: 3))
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
    func testAgentFirstTasksDemoCoversBoundedActionsContextsAndColdRoute() throws {
        let app = launchDemo(extraArguments: ["--ui-test-task-crowded-contexts"])
        openToday(in: app)

        XCTAssertTrue(element("agent-task-today", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(element("task-urgent", in: app).exists)
        XCTAssertTrue(element("task-next-card", in: app).exists)
        XCTAssertTrue(element("task-done-today", in: app).exists)
        XCTAssertFalse(element("task-view-only", in: app).exists)
        XCTAssertLessThanOrEqual(taskRows(in: app).count, 7)
        XCTAssertTrue(
            element(
                "task-complete-019f8800-0000-7000-8000-000000000001",
                in: app
            ).waitForExistence(timeout: 2),
            "The Urgent container must preserve each task-specific action identifier."
        )

        let completionRef = "019f8800-0000-7000-8000-000000000003"
        let complete = element("task-complete-\(completionRef)", in: app)
        XCTAssertTrue(complete.waitForExistence(timeout: 2))
        XCTAssertTrue(complete.isEnabled)
        complete.tap()
        XCTAssertTrue(element("task-row-\(completionRef)", in: app).waitForNonExistence(timeout: 2))

        let snoozeRef = "019f8800-0000-7000-8000-000000000005"
        let snoozeRow = element("task-row-\(snoozeRef)", in: app)
        XCTAssertTrue(snoozeRow.waitForExistence(timeout: 2))
        snoozeRow.press(forDuration: 1.0)
        let threeDays = app.buttons["3 days"]
        XCTAssertTrue(threeDays.waitForExistence(timeout: 2))
        threeDays.tap()
        XCTAssertTrue(snoozeRow.waitForNonExistence(timeout: 2))

        let filteredRef = "019f8800-0000-7000-8000-000000000006"
        XCTAssertTrue(element("task-row-\(filteredRef)", in: app).waitForExistence(timeout: 2))
        tapContextChip(
            app.buttons.matching(identifier: "task-context-online").firstMatch,
            in: app
        )
        XCTAssertTrue(element("task-row-\(filteredRef)", in: app).waitForNonExistence(timeout: 2))

        app.terminate()
        let routeRef = "019f8800-0000-7000-8000-000000000002"
        app.open(try XCTUnwrap(URL(string: "straylight://task/\(routeRef)")))
        XCTAssertTrue(element("task-detail", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(element("task-detail-title", in: app).exists)
    }

    @MainActor
    func testAgentFirstTasksEmptyUrgentAndViewOnlyStates() {
        let emptyApp = launchDemo(extraArguments: ["--ui-test-task-empty-urgent"])
        openToday(in: emptyApp)
        XCTAssertTrue(element("task-urgent-empty", in: emptyApp).waitForExistence(timeout: 3))
        XCTAssertFalse(element("task-urgent", in: emptyApp).exists)
        XCTAssertLessThanOrEqual(taskRows(in: emptyApp).count, 7)
        emptyApp.terminate()

        let viewOnlyApp = launchDemo(extraArguments: ["--ui-test-task-read-only"])
        openToday(in: viewOnlyApp)
        XCTAssertTrue(element("task-view-only", in: viewOnlyApp).waitForExistence(timeout: 3))
        let complete = element(
            "task-complete-019f8800-0000-7000-8000-000000000003",
            in: viewOnlyApp
        )
        XCTAssertTrue(complete.waitForExistence(timeout: 2))
        XCTAssertFalse(complete.isEnabled)
    }

    @MainActor
    func testGate12DAgentFirstTasksAgainstDisposableStack() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let email = environment["STRAYLIGHT_E2E_OWNER_EMAIL"],
              let password = environment["STRAYLIGHT_E2E_OWNER_PASSWORD"],
              let completeRef = environment["STRAYLIGHT_E2E_COMPLETE_TASK_REF"],
              let snoozeRef = environment["STRAYLIGHT_E2E_SNOOZE_TASK_REF"],
              let routeRef = environment["STRAYLIGHT_E2E_ROUTE_TASK_REF"]
        else {
            throw XCTSkip("Disposable-stack owner credentials and seeded task refs were not supplied.")
        }
        let baseURL = environment["STRAYLIGHT_E2E_API_BASE_URL"]
            ?? "http://127.0.0.1:18111/v1"
        let context = environment["STRAYLIGHT_E2E_FILTER_CONTEXT"] ?? "phone"
        let filteredRef = environment["STRAYLIGHT_E2E_FILTERED_TASK_REF"] ?? snoozeRef

        let app = XCUIApplication()
        let localeArguments = [
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryL",
        ]
        app.launchArguments = [
            "--ui-test-connection-required",
            "--ui-test-reset-task-contexts",
        ] + localeArguments
        app.launchEnvironment["TZ"] = "America/Los_Angeles"
        app.launchEnvironment["STRAYLIGHT_API_BASE_URL"] = baseURL
        app.launchEnvironment["STRAYLIGHT_CREDENTIAL_NAMESPACE"] =
            "gate12d-\(UUID().uuidString.lowercased())"
        app.open(try XCTUnwrap(URL(string: "straylight://task/\(routeRef)")))

        let emailField = app.textFields["login-email"]
        XCTAssertTrue(emailField.waitForExistence(timeout: 5))
        emailField.tap()
        emailField.typeText(email)
        let passwordField = app.secureTextFields["login-password"]
        passwordField.tap()
        passwordField.typeText(password)
        app.buttons["Sign in"].tap()
        XCTAssertTrue(element("task-detail", in: app).waitForExistence(timeout: 12))
        XCTAssertTrue(element("task-detail-title", in: app).exists)
        XCTAssertTrue(element("task-detail-view-only", in: app).exists)
        XCUIDevice.shared.press(.home)
        let backgrounded = app.wait(for: .runningBackground, timeout: 5)
            || app.state == .runningBackgroundSuspended
        XCTAssertTrue(backgrounded)
        Thread.sleep(forTimeInterval: 2)
        app.terminate()
        app.launchArguments = ["--ui-test-reset-task-contexts"] + localeArguments
        app.launch()
        XCTAssertTrue(app.tabBars.buttons["Today"].waitForExistence(timeout: 10))

        openToday(in: app)
        XCTAssertTrue(element("agent-task-today", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-urgent", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-next-card", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-done-today", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-view-only", in: app).exists)
        XCTAssertLessThanOrEqual(taskRows(in: app).count, 7)
        let viewOnlyComplete = element("task-complete-\(completeRef)", in: app)
        XCTAssertTrue(viewOnlyComplete.waitForExistence(timeout: 5))
        XCTAssertFalse(viewOnlyComplete.isEnabled)

        let contextChip = app.buttons
            .matching(identifier: "task-context-\(context)")
            .firstMatch
        let contextFilteredRow = element("task-row-\(filteredRef)", in: app)
        XCTAssertTrue(contextChip.waitForExistence(timeout: 3))
        XCTAssertTrue(contextFilteredRow.waitForExistence(timeout: 3))
        tapContextChip(contextChip, in: app)
        XCTAssertTrue(contextFilteredRow.waitForNonExistence(timeout: 5))
        tapContextChip(contextChip, in: app)
        XCTAssertTrue(contextFilteredRow.waitForExistence(timeout: 8))

        selectNativeTab("Settings", in: app)
        let bootstrap = element("device-task-access-bootstrap", in: app)
        scroll(bootstrap, intoViewIn: app)
        bootstrap.tap()
        let revoke = element("device-task-access-revoke", in: app)
        XCTAssertTrue(revoke.waitForExistence(timeout: 10))

        app.tabBars.buttons["Today"].tap()
        let writableComplete = element("task-complete-\(completeRef)", in: app)
        XCTAssertTrue(writableComplete.waitForExistence(timeout: 8))
        XCTAssertTrue(writableComplete.isEnabled)
        writableComplete.tap()
        XCTAssertTrue(element("task-row-\(completeRef)", in: app).waitForNonExistence(timeout: 8))

        let snoozeRow = element("task-row-\(snoozeRef)", in: app)
        XCTAssertTrue(snoozeRow.waitForExistence(timeout: 8))
        snoozeRow.press(forDuration: 1.0)
        let tomorrow = app.buttons["Tomorrow"]
        XCTAssertTrue(tomorrow.waitForExistence(timeout: 3))
        tomorrow.tap()
        XCTAssertTrue(snoozeRow.waitForNonExistence(timeout: 8))

        selectNativeTab("Settings", in: app)
        let finalRevoke = element("device-task-access-revoke", in: app)
        scroll(finalRevoke, intoViewIn: app)
        finalRevoke.tap()
        XCTAssertTrue(element("device-task-access-bootstrap", in: app).waitForExistence(timeout: 8))
    }

    @MainActor
    private func launchDemo(
        contentSizeCategory: String = "UICTContentSizeCategoryL",
        extraArguments: [String] = []
    ) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--demo",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
            "-UIPreferredContentSizeCategoryName", contentSizeCategory,
        ] + extraArguments
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
    private func selectNativeTab(_ label: String, in app: XCUIApplication) {
        let direct = app.tabBars.buttons[label]
        if direct.waitForExistence(timeout: 3) {
            if !direct.isSelected { direct.tap() }
            return
        }

        let more = app.tabBars.buttons["More"]
        XCTAssertTrue(more.waitForExistence(timeout: 3))
        more.tap()
        let destination = app.staticTexts[label].firstMatch
        XCTAssertTrue(destination.waitForExistence(timeout: 3))
        destination.tap()
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
    private func taskRows(in app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "task-row-")
        )
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
    private func tapContextChip(
        _ chip: XCUIElement,
        in app: XCUIApplication,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertTrue(
            chip.waitForExistence(timeout: 3),
            "Context chip does not exist.",
            file: file,
            line: line
        )
        XCTAssertTrue(
            chip.isHittable,
            "Context chip is not directly tappable.",
            file: file,
            line: line
        )
        chip.tap()
    }

    @MainActor
    private func keepScreenshot(named name: String, from app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
