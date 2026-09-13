import XCTest

final class BrunnUITests: XCTestCase {
    @MainActor
    func testComparisonSummaryWrapsAllColumnsAndKeepsFinalDetail() {
        let app = launchDemo(extraArguments: ["--ui-test-review-fixture", "--ui-test-review-table"])
        app.tabBars.buttons["Review"].tap()
        let item = app.buttons["review-item-demo-review-location"]
        XCTAssertTrue(item.waitForExistence(timeout: 4))
        item.tap()
        for value in ["Storage", "Shelves were planned for the narrow wall. [s1]",
                      "The newer measurement leaves enough room for a workbench, with the shelves on the opposite wall. [s2]",
                      "Lighting", "Use separate task lighting above the bench; the final fixture choice remains open. [s4]",
                      "Final detail: the newer measurements replace the old layout assumption."] {
            let text = app.staticTexts[value]
            scroll(text, intoViewIn: app)
            XCTAssertTrue(text.isHittable, "Missing table content: \(value)")
            XCTAssertGreaterThanOrEqual(text.frame.minX, app.frame.minX)
            XCTAssertLessThanOrEqual(text.frame.maxX, app.frame.maxX)
            if value.hasPrefix("The newer") { keepScreenshot(named: "review-comparison-wrapped-columns", from: app) }
        }
        keepScreenshot(named: "review-comparison-final-detail", from: app)
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "| Topic | Earlier information |")).firstMatch.exists)
        let changes = element("review-exact-changes-toggle", in: app)
        scroll(changes, intoViewIn: app)
        changes.tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "| Topic | Earlier information |")).firstMatch.exists)
    }

    @MainActor
    func testReviewIsVisibleInTabBarAndReachableFromHome() {
        let app = launchDemo()
        XCTAssertTrue(app.navigationBars["Home"].waitForExistence(timeout: 5))
        let reviewTab = app.tabBars.buttons["Review"]
        XCTAssertTrue(reviewTab.exists)
        XCTAssertTrue(reviewTab.isHittable)
        let shortcut = app.buttons["dashboard-review"]
        XCTAssertTrue(shortcut.exists)
        shortcut.tap()
        XCTAssertTrue(app.navigationBars["Review"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Sign in to read your Dreamer proposals, evidence, and decisions."].exists)
        keepScreenshot(named: "review-native-tab", from: app)
    }

    @MainActor
    func testNativeReviewReadsCompleteQuestionAndPinnedEvidence() {
        let app = launchDemo(extraArguments: ["--ui-test-review-fixture"])
        app.tabBars.buttons["Review"].tap()
        let item = app.buttons["review-item-demo-review-question"]
        XCTAssertTrue(item.waitForExistence(timeout: 4))
        item.tap()
        XCTAssertTrue(app.navigationBars["Review item"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Report-only: approvals are held; nothing is applied."].exists)
        let finalDetail = app.staticTexts["Final detail: keep the plan in review until I decide."]
        scroll(finalDetail, intoViewIn: app)
        XCTAssertTrue(finalDetail.isHittable)
        keepScreenshot(named: "review-complete-question-end", from: app)
        let evidence = app.buttons["Evidence for the review · v12"]
        XCTAssertFalse(evidence.exists)
        let evidenceDisclosure = element("review-evidence-toggle", in: app)
        scroll(evidenceDisclosure, intoViewIn: app)
        evidenceDisclosure.tap()
        scroll(evidence, intoViewIn: app)
        evidence.tap()
        XCTAssertTrue(app.navigationBars["Entry"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Pinned v12"].exists)
        keepScreenshot(named: "review-exact-evidence", from: app)
        app.navigationBars.buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Review item"].waitForExistence(timeout: 3))
        let saveAnswer = app.buttons["Save answer"]
        scroll(saveAnswer, intoViewIn: app)
        XCTAssertFalse(saveAnswer.isEnabled)
        XCTAssertFalse(app.buttons["Approve"].exists)
        XCTAssertFalse(app.staticTexts["Candidate"].exists)
        app.navigationBars.buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Review"].waitForExistence(timeout: 3))
        XCTAssertTrue(item.exists)
    }

    @MainActor
    func testReviewRefreshesAnOpenDetailOnForegroundAndRequiresUpdatedAcknowledgment() {
        let app = launchDemo(extraArguments: ["--ui-test-review-fixture", "--ui-test-review-location", "--ui-test-review-replacement"])
        app.tabBars.buttons["Review"].tap()
        let item = app.buttons["review-item-demo-review-location"]
        XCTAssertTrue(item.waitForExistence(timeout: 4))
        item.tap()
        XCTAssertTrue(app.staticTexts["February 3 · UTC"].waitForExistence(timeout: 3))
        XCUIDevice.shared.press(.home)
        app.activate()
        let updated = app.staticTexts["The replacement summary arrived while this item was open."]
        XCTAssertTrue(updated.waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["February 3 · UTC"].exists)
        XCTAssertTrue(app.staticTexts["This review has changed"].exists)
        let acknowledge = app.buttons["Review updated item"]
        XCTAssertTrue(acknowledge.exists)
        scroll(acknowledge, intoViewIn: app)
        acknowledge.tap()
        XCTAssertFalse(app.staticTexts["This review has changed"].exists)
        XCTAssertTrue(updated.exists)
        keepScreenshot(named: "review-refreshed-open-detail", from: app)
    }

    @MainActor
    func testManagedLocationSummaryShowsSevenReadableStopsAndHidesAuditDetails() {
        let app = launchDemo(extraArguments: ["--ui-test-review-fixture", "--ui-test-review-location"])
        app.tabBars.buttons["Review"].tap()
        let item = app.buttons["review-item-demo-review-location"]
        XCTAssertTrue(item.waitForExistence(timeout: 4))
        item.tap()
        XCTAssertTrue(app.navigationBars["Review item"].waitForExistence(timeout: 3))
        for value in ["About 06:11–08:16", "Library", "About 10:41–10:44", "Cafe, brief stop",
                      "About 11:08–16:32", "Community center", "Market", "About 21:27–21:28", "Station entrance"] {
            let text = app.staticTexts[value]
            scroll(text, intoViewIn: app)
            XCTAssertTrue(text.isHittable, "Missing readable stop: \(value)")
            if value == "Cafe, brief stop" { keepScreenshot(named: "review-location-readable-stops", from: app) }
        }
        XCTAssertFalse(app.staticTexts["Before"].exists)
        XCTAssertFalse(app.staticTexts["After"].exists)
        XCTAssertFalse(app.staticTexts["Why it is proposed"].exists)
        XCTAssertFalse(app.staticTexts["Uncertainty"].exists)
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "| When | Where |")).firstMatch.exists)
        XCTAssertFalse(app.buttons["Location evidence 1 · v12"].exists)
        let approve = app.buttons["Approve"]
        scroll(approve, intoViewIn: app)
        XCTAssertTrue(approve.isHittable)
        XCTAssertFalse(approve.isEnabled)
        keepScreenshot(named: "review-location-decision-with-collapsed-evidence", from: app)
        let changes = element("review-exact-changes-toggle", in: app)
        scroll(changes, intoViewIn: app)
        changes.tap()
        XCTAssertTrue(app.staticTexts["Before"].waitForExistence(timeout: 2))
        XCTAssertTrue(app.staticTexts["Previous location summary."].exists)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "| When | Where |")).firstMatch.exists)
    }

    @MainActor
    func testOlderReportsAreCollapsedReadOnlyAndSeparateFromReview() {
        let app = launchDemo(extraArguments: ["--ui-test-review-fixture"])
        app.tabBars.buttons["Review"].tap()
        XCTAssertTrue(app.buttons["review-item-demo-review-question"].waitForExistence(timeout: 4))
        XCTAssertTrue(caseInsensitiveText("1 for review", in: app).exists)
        XCTAssertFalse(app.buttons["review-item-demo-review-legacy"].exists)
        XCTAssertFalse(app.buttons["review-older-demo-review-legacy"].exists)
        let disclosure = element("review-older-reports", in: app)
        scroll(disclosure, intoViewIn: app)
        disclosure.tap()
        let note = app.buttons["review-older-demo-review-legacy"]
        scroll(note, intoViewIn: app)
        note.tap()
        XCTAssertTrue(app.navigationBars["Older report"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Kept for reference. This note is not awaiting a decision and will not be applied."].exists)
        XCTAssertTrue(app.staticTexts["Original note"].exists)
        XCTAssertTrue(app.staticTexts["Old promise from an earlier run"].exists)
        XCTAssertFalse(app.staticTexts["Applies next run unless vetoed: Old promise from an earlier run"].exists)
        XCTAssertFalse(app.staticTexts["Candidate"].exists)
        XCTAssertFalse(app.staticTexts["Before"].exists)
        XCTAssertFalse(app.staticTexts["After"].exists)
        XCTAssertFalse(app.buttons["Approve"].exists)
        XCTAssertFalse(app.buttons["Reject"].exists)
        XCTAssertFalse(app.buttons["Defer"].exists)
        XCTAssertFalse(app.buttons["Save correction"].exists)
        XCTAssertFalse(app.buttons["Save answer"].exists)
        keepScreenshot(named: "review-readonly-older-report", from: app)
        app.navigationBars.buttons.firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Review"].waitForExistence(timeout: 3))
        XCTAssertTrue(caseInsensitiveText("1 for review", in: app).exists)
    }

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
        XCTAssertFalse(element("brunn-startup", in: app).exists)
    }

    @MainActor
    func testAuthenticatedInstallShowsLocationPermissionPrimerOnce() {
        let app = XCUIApplication()
        app.resetAuthorizationStatus(for: .location)
        app.launchArguments = [
            "--ui-test-location-permission-prompt",
            "--ui-test-reset-location-prompt",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
        ]

        app.launch()

        let primer = element("location-permission-primer", in: app)
        XCTAssertTrue(primer.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Let Brunn know where you are"].exists)
        let notNow = app.buttons["location-permission-not-now"]
        XCTAssertTrue(notNow.exists)
        notNow.tap()
        XCTAssertTrue(primer.waitForNonExistence(timeout: 2))

        app.terminate()
        app.launchArguments = [
            "--ui-test-location-permission-prompt",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
        ]
        app.launch()

        XCTAssertTrue(app.navigationBars["Home"].waitForExistence(timeout: 5))
        XCTAssertFalse(
            element("location-permission-primer", in: app).waitForExistence(timeout: 2),
            "The one-time location primer appeared again after dismissal."
        )
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
        XCTAssertFalse(app.staticTexts["Your Brunn"].exists)
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
        field.typeText("Brunn")
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

        let linkedEntry = app.links["Brunn Briefings: Platform Design"]
        XCTAssertTrue(linkedEntry.waitForExistence(timeout: 2))
        scroll(linkedEntry, intoViewIn: app)
        linkedEntry.tap()
        XCTAssertTrue(
            caseInsensitiveText("Brunn Briefings: Platform Design", in: app)
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
    func testDemoReaderUsesThePhoneWidthAndExpandsOneItemAtATime() {
        let app = launchDemo()
        openToday(in: app)

        XCTAssertTrue(app.navigationBars["Today"].waitForExistence(timeout: 5))

        let reader = element("briefing-reader", in: app)
        let title = element("briefing-reader-title", in: app)
        let items = element("briefing-items", in: app)
        let first = element("briefing-item-ios-direction", in: app)
        let second = element("briefing-item-existing-contracts", in: app)
        XCTAssertTrue(reader.waitForExistence(timeout: 3))
        XCTAssertTrue(title.exists)
        XCTAssertTrue(items.exists)
        XCTAssertTrue(first.exists)
        XCTAssertFalse(app.staticTexts["30-SECOND SUMMARY"].exists)

        XCTAssertLessThanOrEqual(
            items.frame.minX - app.frame.minX,
            20,
            "The briefing list should not retain a timeline gutter on compact screens."
        )
        XCTAssertGreaterThanOrEqual(
            items.frame.width / app.frame.width,
            0.90,
            "The briefing list should use at least 90% of the phone width."
        )
        XCTAssertFalse(element("briefing-item-detail-ios-direction", in: app).exists, "The reader loads collapsed.")

        scroll(first, intoViewIn: app)
        first.tap()
        XCTAssertTrue(element("briefing-item-detail-ios-direction", in: app).waitForExistence(timeout: 2))

        scroll(second, intoViewIn: app)
        second.tap()
        XCTAssertTrue(element("briefing-item-detail-existing-contracts", in: app).waitForExistence(timeout: 2))
        XCTAssertTrue(
            element("briefing-item-detail-ios-direction", in: app).waitForNonExistence(timeout: 2),
            "Expanding one item collapses the other."
        )

        second.tap()
        XCTAssertTrue(element("briefing-item-detail-existing-contracts", in: app).waitForNonExistence(timeout: 2))

        keepScreenshot(named: "briefing-reader-accordion", from: app)
    }

    @MainActor
    func testEveryDemoSectionAndItemDetailIsReachableWithSourcesAndHistory() {
        let app = launchDemo()
        openToday(in: app)
        XCTAssertTrue(element("briefing-reader", in: app).waitForExistence(timeout: 5))

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
        selectNativeTab("Settings", in: app)

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
        selectNativeTab("Settings", in: app)

        let restoredAppearance = app.segmentedControls["appearance-mode"]
        XCTAssertTrue(restoredAppearance.waitForExistence(timeout: 2))
        XCTAssertTrue(restoredAppearance.buttons["Light"].isSelected)
    }

    @MainActor
    func testArchiveOpensPriorEditionsAndPinnedRevision() {
        let app = launchDemo()
        selectNativeTab("Archive", in: app)

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
    func testAgentFirstTasksDemoCoversDedicatedTabActionsAndColdRoute() throws {
        let app = launchDemo(extraArguments: ["--ui-test-task-crowded-contexts"])
        openToday(in: app)
        XCTAssertTrue(element("briefing-reader", in: app).waitForExistence(timeout: 5))
        XCTAssertFalse(element("agent-task-surface", in: app).exists)
        openTasks(in: app)

        XCTAssertTrue(element("agent-task-surface", in: app).waitForExistence(timeout: 5))
        XCTAssertFalse(element("task-contexts-card", in: app).exists)
        XCTAssertTrue(element("task-projects", in: app).exists)
        XCTAssertTrue(element("task-project-charlemagne", in: app).exists)
        XCTAssertTrue(element("task-urgent", in: app).exists)
        XCTAssertTrue(element("task-today", in: app).exists)
        XCTAssertTrue(element("task-quick", in: app).exists)
        XCTAssertTrue(element("task-next-card", in: app).exists)
        XCTAssertTrue(element("task-done-today", in: app).exists)
        XCTAssertFalse(element("task-view-only", in: app).exists)
        XCTAssertLessThanOrEqual(taskRows(in: app).count, 7)
        let todoistRow = element("task-row-019f8800-0000-7000-8000-000000000005", in: app)
        XCTAssertTrue(todoistRow.waitForExistence(timeout: 2))
        XCTAssertTrue(todoistRow.label.contains("Imported from Todoist"))
        keepScreenshot(named: "tasks-dedicated-tab", from: app)
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
        snoozeRow.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 1.0)
        let threeDays = app.buttons["3 days"]
        XCTAssertTrue(threeDays.waitForExistence(timeout: 2))
        threeDays.tap()
        XCTAssertTrue(snoozeRow.waitForNonExistence(timeout: 2))

        app.terminate()
        let routeRef = "019f8800-0000-7000-8000-000000000002"
        app.open(try XCTUnwrap(URL(string: "brunn://task/\(routeRef)")))
        XCTAssertTrue(element("task-detail", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(element("task-detail-title", in: app).exists)
    }

    @MainActor
    func testTaskTimingNavigationAndNextHierarchy() {
        let app = launchDemo()
        openTasks(in: app)
        let next = element("task-next-card", in: app)
        XCTAssertTrue(next.waitForExistence(timeout: 5))
        let quick = element("task-quick", in: app)
        XCTAssertLessThan(next.frame.minY, quick.frame.minY)
        let timing = element("task-timing-open", in: app)
        XCTAssertTrue(timing.isHittable)
        timing.tap()
        XCTAssertTrue(app.navigationBars["Timing-sensitive"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Upcoming, deferred, and timing to clarify. Unknown timing does not schedule a reminder."].exists)
        app.buttons["Done"].tap()
        let row = element("task-row-019f8800-0000-7000-8000-000000000003", in: app)
        scroll(row, intoViewIn: app)
        row.tap()
        XCTAssertTrue(element("task-detail", in: app).waitForExistence(timeout: 3))
        let editor = app.buttons["Timing and recurrence"]
        scroll(editor, intoViewIn: app)
        XCTAssertTrue(editor.exists)
        editor.tap()
        let save = app.buttons["Save intended date"]
        scroll(save, intoViewIn: app)
        XCTAssertTrue(save.isHittable)
        keepScreenshot(named: "task-timing-editor", from: app)
    }

    @MainActor
    func testTaskDeletionRequiresConfirmationAndDoesNotCompleteTask() {
        let app = launchDemo()
        openTasks(in: app)
        let taskRef = "019f8800-0000-7000-8000-000000000003"
        let row = element("task-row-\(taskRef)", in: app)
        let doneCount = element("task-done-today", in: app).label
        row.tap()
        let delete = element("task-detail-delete", in: app)
        XCTAssertTrue(delete.waitForExistence(timeout: 3))
        delete.tap()
        app.buttons["Cancel"].tap()
        XCTAssertTrue(delete.exists)
        delete.tap()
        let confirmation = app.alerts.buttons["Delete task"]
        XCTAssertTrue(confirmation.waitForExistence(timeout: 3))
        confirmation.tap()
        XCTAssertTrue(element("task-detail", in: app).waitForNonExistence(timeout: 3))
        XCTAssertTrue(row.waitForNonExistence(timeout: 3))
        XCTAssertEqual(element("task-done-today", in: app).label, doneCount)

        let otherRow = element("task-row-019f8800-0000-7000-8000-000000000005", in: app)
        otherRow.press(forDuration: 1)
        app.buttons["Delete task"].tap()
        XCTAssertTrue(confirmation.waitForExistence(timeout: 3))
        confirmation.tap()
        XCTAssertTrue(otherRow.waitForNonExistence(timeout: 3))
    }

    @MainActor
    func testAgentFirstTasksEmptyUrgentAndViewOnlyStates() {
        let emptyApp = launchDemo(extraArguments: ["--ui-test-task-empty-urgent"])
        openTasks(in: emptyApp)
        XCTAssertTrue(element("task-next-card", in: emptyApp).waitForExistence(timeout: 3))
        XCTAssertFalse(element("task-urgent-empty", in: emptyApp).exists)
        XCTAssertFalse(element("task-urgent", in: emptyApp).exists)
        XCTAssertLessThanOrEqual(taskRows(in: emptyApp).count, 7)
        emptyApp.terminate()

        let viewOnlyApp = launchDemo(extraArguments: ["--ui-test-task-read-only"])
        openTasks(in: viewOnlyApp)
        XCTAssertTrue(element("task-view-only", in: viewOnlyApp).waitForExistence(timeout: 3))
        XCTAssertFalse(element("task-enable-actions", in: viewOnlyApp).exists)
        let complete = element(
            "task-complete-019f8800-0000-7000-8000-000000000003",
            in: viewOnlyApp
        )
        XCTAssertTrue(complete.waitForExistence(timeout: 2))
        XCTAssertFalse(complete.isEnabled)
        let capture = element("task-today-capture", in: viewOnlyApp)
        XCTAssertTrue(capture.exists)
        XCTAssertFalse(capture.isEnabled)
        element("task-row-019f8800-0000-7000-8000-000000000003", in: viewOnlyApp).tap()
        XCTAssertTrue(element("task-detail-view-only", in: viewOnlyApp).waitForExistence(timeout: 3))
        XCTAssertFalse(element("task-detail-delete", in: viewOnlyApp).exists)
        viewOnlyApp.terminate()
    }

    @MainActor
    func testGate12DAgentFirstTasksAgainstDisposableStack() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let email = environment["BRUNN_E2E_OWNER_EMAIL"],
              let password = environment["BRUNN_E2E_OWNER_PASSWORD"],
              let completeRef = environment["BRUNN_E2E_COMPLETE_TASK_REF"],
              let snoozeRef = environment["BRUNN_E2E_SNOOZE_TASK_REF"],
              let routeRef = environment["BRUNN_E2E_ROUTE_TASK_REF"]
        else {
            throw XCTSkip("Disposable-stack owner credentials and seeded task refs were not supplied.")
        }
        let baseURL = environment["BRUNN_E2E_API_BASE_URL"]
            ?? "http://127.0.0.1:18111/v1"
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
        app.launchEnvironment["BRUNN_API_BASE_URL"] = baseURL
        app.launchEnvironment["BRUNN_CREDENTIAL_NAMESPACE"] =
            "gate12d-\(UUID().uuidString.lowercased())"
        app.open(try XCTUnwrap(URL(string: "brunn://task/\(routeRef)")))

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
        XCTAssertTrue(app.tabBars.buttons["Tasks"].waitForExistence(timeout: 10))

        openTasks(in: app)
        XCTAssertTrue(element("agent-task-surface", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-urgent", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-next-card", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-done-today", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("task-view-only", in: app).exists)
        XCTAssertLessThanOrEqual(taskRows(in: app).count, 7)
        let viewOnlyComplete = element("task-complete-\(completeRef)", in: app)
        XCTAssertTrue(viewOnlyComplete.waitForExistence(timeout: 5))
        XCTAssertFalse(viewOnlyComplete.isEnabled)

        XCTAssertFalse(element("task-contexts-card", in: app).exists)
        let enableActions = element("task-enable-actions", in: app)
        XCTAssertTrue(enableActions.waitForExistence(timeout: 3))
        enableActions.tap()
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
    private func openTasks(in app: XCUIApplication) {
        let tasks = app.tabBars.buttons["Tasks"]
        XCTAssertTrue(tasks.waitForExistence(timeout: 3))
        tasks.tap()
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
    private func keepScreenshot(named name: String, from app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
