import Foundation
import XCTest

final class AgentMessagingUITests: XCTestCase {
    private struct Fixture {
        let email: String
        let password: String
        let baseURL: String
        let unusedBaseURL: String
        let conversationID: String
        let echoAgentID: String
        let routeSequence: Int

        static func load() throws -> Self {
            let environment = ProcessInfo.processInfo.environment
            guard let email = environment["BRUNN_E2E_OWNER_EMAIL"],
                  let password = environment["BRUNN_E2E_OWNER_PASSWORD"],
                  let conversationID = environment[
                      "BRUNN_E2E_MESSAGING_CONVERSATION_ID"
                  ],
                  let echoAgentID = environment["BRUNN_E2E_MESSAGING_ECHO_AGENT_ID"]
            else {
                throw XCTSkip(
                    "Disposable-stack owner credentials and opaque messaging fixture IDs were not supplied."
                )
            }

            guard AgentMessagingUITests.isCanonicalUUIDv7(conversationID),
                  AgentMessagingUITests.isOpaquePrincipalID(echoAgentID)
            else {
                throw XCTSkip("The messaging fixture identifiers are not canonical.")
            }
            let routeSequence = Int(
                environment["BRUNN_E2E_MESSAGING_ROUTE_SEQ"] ?? "1"
            ) ?? 0
            guard routeSequence > 0 else {
                throw XCTSkip("The seeded messaging route sequence must be positive.")
            }

            return Self(
                email: email,
                password: password,
                baseURL: environment["BRUNN_E2E_API_BASE_URL"]
                    ?? "http://127.0.0.1:18112/v1",
                unusedBaseURL: environment["BRUNN_E2E_UNUSED_API_BASE_URL"]
                    ?? "http://127.0.0.1:58112/v1",
                conversationID: conversationID,
                echoAgentID: echoAgentID,
                routeSequence: routeSequence
            )
        }
    }

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// Gate 12(c): one real disposable stack, one protected device credential,
    /// one account-bound local store, and no bearer material in app arguments,
    /// environment, screenshots, or attachments.
    @MainActor
    func testGate12CAgentMessagingAgainstDisposableStack() throws {
        let fixture = try Fixture.load()
        XCTAssertNotEqual(
            fixture.baseURL,
            fixture.unusedBaseURL,
            "The offline phase must point at a distinct unused localhost port."
        )
        let namespace = "gate12c-\(UUID().uuidString.lowercased())"
        let routeURL = try conversationURL(
            conversationID: fixture.conversationID,
            sequence: fixture.routeSequence
        )
        let app = configuredApp(
            baseURL: fixture.baseURL,
            namespace: namespace,
            requireSignIn: true
        )

        // A cold route is retained across the signed-out boundary and opens the
        // exact durable message after login.
        app.open(routeURL)
        signIn(app, email: fixture.email, password: fixture.password)
        XCTAssertTrue(
            element("messaging-thread-\(fixture.conversationID)", in: app)
                .waitForExistence(timeout: 15)
        )
        assertMessageIsVisible(
            conversationID: fixture.conversationID,
            sequence: fixture.routeSequence,
            in: app
        )

        // The cached Agents list and the same thread both open before a device
        // write credential exists. Reading remains available and composing is
        // explicitly view-only.
        openConversationList(fromThreadIn: app)
        XCTAssertTrue(element("messaging-agents-root", in: app).exists)
        XCTAssertTrue(element("messaging-conversation-list", in: app).exists)
        openConversation(fixture.conversationID, in: app)
        let viewOnlyNotice = element("messaging-view-only", in: app)
        XCTAssertTrue(viewOnlyNotice.waitForExistence(timeout: 3))
        XCTAssertEqual(
            viewOnlyNotice.label,
            "View only. Open More, then Settings, to add secure message access."
        )

        // Settings creates the same one-time device credential used by Tasks.
        // The fixture has granted its exact least-privilege template the one
        // additive message.write capability; no second credential is created.
        selectNativeTab("Settings", in: app)
        let bootstrap = element("device-task-access-bootstrap", in: app)
        scroll(bootstrap, intoViewIn: app)
        bootstrap.tap()
        let capabilities = element("messaging-device-access-capabilities", in: app)
        XCTAssertTrue(capabilities.waitForExistence(timeout: 12))
        XCTAssertEqual(
            capabilities.label,
            "task.write + message.write + notification.manage only"
        )

        selectNativeTab("Agents", in: app)
        openConversation(fixture.conversationID, in: app)
        XCTAssertTrue(element("messaging-view-only", in: app).waitForNonExistence(timeout: 8))
        XCTAssertTrue(element("messaging-composer", in: app).isEnabled)

        // A question paints optimistically before its canonical acknowledgement,
        // then exactly one reply from the seeded echo principal arrives.
        let runID = UUID().uuidString.lowercased().prefix(8)
        let onlineBody = "Gate 12c question \(runID)"
        let initialMessageCount = try loadedMessageCount(
            conversationID: fixture.conversationID,
            in: app
        )
        sendQuestion(onlineBody, in: app)
        XCTAssertTrue(
            exactText(onlineBody, in: app).firstMatch.waitForExistence(timeout: 1),
            "The owner message did not paint optimistically in the send turn."
        )
        XCTAssertTrue(
            pendingOutboxRows(in: app).count > 0 || exactText(onlineBody, in: app).firstMatch.exists,
            "The optimistic message vanished before canonical acknowledgement."
        )
        XCTAssertTrue(waitUntil(timeout: 15) {
            (try? self.loadedMessageCount(
                conversationID: fixture.conversationID,
                in: app
            )) == initialMessageCount + 2
        })
        XCTAssertTrue(waitUntil(timeout: 8) { pendingOutboxRows(in: app).count == 0 })

        // The generic conversation deep link focuses the exact echo sequence;
        // no push payload text is needed to route it.
        let echoTarget = try latestWireTarget(from: fixture.echoAgentID, in: app)
        // A system-opened URL does not carry XCUIApplication's disposable
        // launch environment. Cold-launch with that environment first, then
        // deliver the route through the real system URL dispatcher.
        app.terminate()
        configure(
            app,
            baseURL: fixture.baseURL,
            namespace: namespace,
            requireSignIn: false
        )
        app.launch()
        XCUIDevice.shared.system.open(try conversationURL(
            conversationID: echoTarget.conversationID,
            sequence: echoTarget.sequence
        ))
        assertMessageIsVisible(
            conversationID: echoTarget.conversationID,
            sequence: echoTarget.sequence,
            in: app
        )

        let messageCountBeforeOfflineSend = try loadedMessageCount(
            conversationID: fixture.conversationID,
            in: app
        )
        app.terminate()

        // Reuse both namespaces while the network points at an unused localhost
        // port. Cached Agents/thread content must paint without a spinner.
        configure(
            app,
            baseURL: fixture.unusedBaseURL,
            namespace: namespace,
            requireSignIn: false
        )
        let offlineLaunchStarted = Date()
        app.launch()
        XCTAssertTrue(app.tabBars.buttons["Agents"].waitForExistence(timeout: 3))
        XCTAssertLessThan(
            Date().timeIntervalSince(offlineLaunchStarted),
            3.5,
            "The protected cached surface did not become available promptly while offline."
        )
        selectNativeTab("Agents", in: app)
        XCTAssertTrue(
            element("messaging-conversation-\(fixture.conversationID)", in: app)
                .waitForExistence(timeout: 3)
        )
        XCTAssertFalse(element("messaging-loading-spinner", in: app).exists)
        openConversation(fixture.conversationID, in: app)
        XCTAssertTrue(
            element("messaging-thread-\(fixture.conversationID)", in: app)
                .waitForExistence(timeout: 2)
        )
        XCTAssertFalse(element("messaging-loading-spinner", in: app).exists)

        let offlineBody = "Gate 12c queued question \(runID)"
        sendQuestion(offlineBody, in: app)
        XCTAssertTrue(exactText(offlineBody, in: app).firstMatch.waitForExistence(timeout: 1))
        XCTAssertTrue(waitUntil(timeout: 6) { queuedOutboxRows(in: app).count == 1 })
        XCTAssertEqual(exactText(offlineBody, in: app).count, 1)

        // The exact persisted request and client ULID survive an offline process
        // death. Relaunching cannot mint a second logical send.
        app.terminate()
        configure(
            app,
            baseURL: fixture.unusedBaseURL,
            namespace: namespace,
            requireSignIn: false
        )
        app.launch()
        selectNativeTab("Agents", in: app)
        openConversation(fixture.conversationID, in: app)
        XCTAssertTrue(queuedOutboxRows(in: app).firstMatch.waitForExistence(timeout: 3))
        XCTAssertEqual(exactText(offlineBody, in: app).count, 1)

        // Restoring the real API retries the unchanged payload once. The local
        // row is reconciled with one canonical owner message and one echo reply;
        // no queued duplicate remains after a short settle interval.
        app.terminate()
        configure(
            app,
            baseURL: fixture.baseURL,
            namespace: namespace,
            requireSignIn: false
        )
        app.launch()
        selectNativeTab("Agents", in: app)
        openConversation(fixture.conversationID, in: app)
        XCTAssertTrue(waitUntil(timeout: 20) { queuedOutboxRows(in: app).count == 0 })
        XCTAssertTrue(waitUntil(timeout: 20) {
            exactText(offlineBody, in: app).count == 1
                && (try? self.loadedMessageCount(
                    conversationID: fixture.conversationID,
                    in: app
                )) == messageCountBeforeOfflineSend + 2
        })
        RunLoop.current.run(until: Date().addingTimeInterval(1.5))
        XCTAssertEqual(exactText(offlineBody, in: app).count, 1)
        XCTAssertEqual(
            try loadedMessageCount(conversationID: fixture.conversationID, in: app),
            messageCountBeforeOfflineSend + 2
        )
        XCTAssertEqual(pendingOutboxRows(in: app).count, 0)
    }

    /// Run first against the gate-off disposable API. This seeds the legacy
    /// exact-two credential into a stable protected namespace without ever
    /// moving its bearer through the UI-test process.
    @MainActor
    func testGate12CLegacyASeedExactTwoCapabilityCredential() throws {
        let fixture = try Fixture.load()
        guard let offBaseURL = ProcessInfo.processInfo.environment[
            "BRUNN_E2E_MESSAGING_OFF_API_BASE_URL"
        ], !offBaseURL.isEmpty else {
            throw XCTSkip("A messaging-off disposable API base URL was not supplied.")
        }
        let namespace = try legacyCredentialNamespace()

        let gateOffApp = configuredApp(
            baseURL: offBaseURL,
            namespace: namespace,
            requireSignIn: true
        )
        gateOffApp.launch()
        signIn(gateOffApp, email: fixture.email, password: fixture.password)
        selectNativeTab("Settings", in: gateOffApp)
        let legacyCapabilities = element(
            "messaging-device-access-capabilities",
            in: gateOffApp
        )
        if !legacyCapabilities.waitForExistence(timeout: 2) {
            let bootstrap = element("device-task-access-bootstrap", in: gateOffApp)
            scroll(bootstrap, intoViewIn: gateOffApp)
            bootstrap.tap()
        }
        XCTAssertTrue(legacyCapabilities.waitForExistence(timeout: 12))
        XCTAssertEqual(
            legacyCapabilities.label,
            "task.write + notification.manage only"
        )
        gateOffApp.terminate()
    }

    /// Run second after the same disposable API restarts gate-on. The exact-two
    /// credential remains useful for Tasks and notifications but cannot write
    /// agent messages, so the cached thread must be view only.
    @MainActor
    func testGate12CLegacyBExactTwoCapabilityCredentialIsViewOnly() throws {
        let fixture = try Fixture.load()
        let namespace = try legacyCredentialNamespace()

        let app = configuredApp(
            baseURL: fixture.baseURL,
            namespace: namespace,
            requireSignIn: true
        )
        app.open(try conversationURL(
            conversationID: fixture.conversationID,
            sequence: fixture.routeSequence
        ))
        signIn(app, email: fixture.email, password: fixture.password)

        XCTAssertTrue(
            element("messaging-thread-\(fixture.conversationID)", in: app)
                .waitForExistence(timeout: 15)
        )
        XCTAssertTrue(element("messaging-view-only", in: app).waitForExistence(timeout: 5))
        let composer = element("messaging-composer", in: app)
        XCTAssertTrue(composer.waitForExistence(timeout: 3))
        XCTAssertFalse(composer.isEnabled)
        XCTAssertTrue(
            element(
                "messaging-message-\(fixture.conversationID)-\(fixture.routeSequence)",
                in: app
            ).exists,
            "The legacy credential must retain cached/read access while messaging stays view-only."
        )
    }

    private func legacyCredentialNamespace() throws -> String {
        guard let namespace = ProcessInfo.processInfo.environment[
            "BRUNN_E2E_MESSAGING_LEGACY_CREDENTIAL_NAMESPACE"
        ], namespace.range(
            of: #"\A[A-Za-z0-9_-]{1,64}\z"#,
            options: .regularExpression
        ) != nil else {
            throw XCTSkip("A stable protected legacy credential namespace was not supplied.")
        }
        return namespace
    }

    /// A separate flag-off disposable API proves that a fresh account-bound
    /// namespace does not acquire or reveal the Agents surface.
    @MainActor
    func testGate12CMessagingOffAgainstDisposableStack() throws {
        let fixture = try Fixture.load()
        guard let offBaseURL = ProcessInfo.processInfo.environment[
            "BRUNN_E2E_MESSAGING_OFF_API_BASE_URL"
        ], !offBaseURL.isEmpty else {
            throw XCTSkip("A messaging-off disposable API base URL was not supplied.")
        }
        let namespace = "gate12c-off-\(UUID().uuidString.lowercased())"
        let app = configuredApp(
            baseURL: offBaseURL,
            namespace: namespace,
            requireSignIn: true
        )
        app.launch()
        signIn(app, email: fixture.email, password: fixture.password)
        assertExactLegacyTabs(in: app)
        XCTAssertFalse(element("messaging-agents-root", in: app).exists)
    }

    /// Deterministic no-network regression for the default-off launch shape.
    @MainActor
    func testDemoMessagingGateOffKeepsTasksVisibleInTheFiveTabBudget() {
        let app = XCUIApplication()
        app.launchArguments = localeArguments + ["--demo"]
        app.launchEnvironment["TZ"] = "America/Los_Angeles"
        app.launch()
        assertExactLegacyTabs(in: app)
        XCTAssertFalse(element("messaging-agents-root", in: app).exists)
    }

    /// The fixture is a logical thread with 1,000 loaded rows (continuations are
    /// transparent to the reader). The count marker is accessibility metadata,
    /// not another screen or production-only test surface.
    @available(iOS 26.0, *)
    @MainActor
    func testGate12CThousandMessageScrollProfile() throws {
        let fixture = try Fixture.load()
        let environment = ProcessInfo.processInfo.environment
        guard let conversationID = environment[
            "BRUNN_E2E_MESSAGING_1000_CONVERSATION_ID"
        ], Self.isCanonicalUUIDv7(conversationID) else {
            throw XCTSkip("The opaque 1,000-message fixture conversation ID was not supplied.")
        }
        let routeSequence = Int(
            environment["BRUNN_E2E_MESSAGING_1000_ROUTE_SEQ"] ?? "1"
        ) ?? 0
        guard routeSequence > 0 else {
            throw XCTSkip("The 1,000-message fixture route sequence must be positive.")
        }

        let app = configuredApp(
            baseURL: fixture.baseURL,
            namespace: "gate12c-1000-\(UUID().uuidString.lowercased())",
            requireSignIn: true
        )
        app.open(try conversationURL(
            conversationID: conversationID,
            sequence: routeSequence
        ))
        signIn(app, email: fixture.email, password: fixture.password)
        XCTAssertTrue(
            element("messaging-thread-\(conversationID)", in: app)
                .waitForExistence(timeout: 20)
        )
        XCTAssertTrue(
            waitUntil(timeout: 30) {
                (try? self.loadedMessageCount(conversationID: conversationID, in: app)) == 1_000
            },
            "The scroll fixture did not report exactly 1,000 loaded message rows."
        )
        XCTAssertEqual(
            try loadedMessageCount(conversationID: conversationID, in: app),
            1_000
        )
        let threadScroll = element("messaging-thread-scroll", in: app)
        XCTAssertTrue(element("messaging-closed", in: app).waitForExistence(timeout: 3))
        XCTAssertTrue(threadScroll.waitForExistence(timeout: 5))

        measure(metrics: [
            XCTOSSignpostMetric.scrollDecelerationMetric,
            XCTHitchMetric(application: app),
        ]) {
            threadScroll.swipeUp(velocity: .fast)
        }
    }

    @MainActor
    private func configuredApp(
        baseURL: String,
        namespace: String,
        requireSignIn: Bool
    ) -> XCUIApplication {
        let app = XCUIApplication()
        configure(
            app,
            baseURL: baseURL,
            namespace: namespace,
            requireSignIn: requireSignIn
        )
        return app
    }

    @MainActor
    private func configure(
        _ app: XCUIApplication,
        baseURL: String,
        namespace: String,
        requireSignIn: Bool
    ) {
        app.launchArguments = localeArguments
            + (requireSignIn ? ["--ui-test-connection-required"] : [])
        app.launchEnvironment = [
            "TZ": "America/Los_Angeles",
            "BRUNN_API_BASE_URL": baseURL,
            "BRUNN_CREDENTIAL_NAMESPACE": namespace,
            "BRUNN_MESSAGING_STORE_NAMESPACE": namespace,
        ]
    }

    private var localeArguments: [String] {
        [
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryL",
        ]
    }

    @MainActor
    private func signIn(_ app: XCUIApplication, email: String, password: String) {
        let emailField = app.textFields["login-email"]
        XCTAssertTrue(emailField.waitForExistence(timeout: 8))
        emailField.tap()
        emailField.typeText(email)
        let passwordField = app.secureTextFields["login-password"]
        XCTAssertTrue(passwordField.exists)
        passwordField.tap()
        passwordField.typeText(password)
        app.buttons["Sign in"].tap()
        let passwordSaveDismissal = app.buttons["Not Now"]
        if passwordSaveDismissal.waitForExistence(timeout: 3) {
            passwordSaveDismissal.tap()
        }
    }

    @MainActor
    private func openConversationList(fromThreadIn app: XCUIApplication) {
        selectNativeTab("Agents", in: app)
        let nativeBack = app.navigationBars.buttons["Agents"]
        if nativeBack.exists {
            nativeBack.tap()
        }
        XCTAssertTrue(element("messaging-conversation-list", in: app).waitForExistence(timeout: 5))
    }

    @MainActor
    private func openConversation(_ conversationID: String, in app: XCUIApplication) {
        let thread = element("messaging-thread-\(conversationID)", in: app)
        if thread.exists { return }
        let row = element("messaging-conversation-\(conversationID)", in: app)
        XCTAssertTrue(row.waitForExistence(timeout: 8))
        row.tap()
        XCTAssertTrue(thread.waitForExistence(timeout: 8))
    }

    @MainActor
    private func sendQuestion(_ body: String, in app: XCUIApplication) {
        let composer = element("messaging-composer", in: app)
        XCTAssertTrue(composer.waitForExistence(timeout: 5))
        XCTAssertTrue(composer.isEnabled)
        composer.tap()
        composer.typeText(body)

        let question = element("messaging-compose-question", in: app)
        XCTAssertTrue(question.waitForExistence(timeout: 3))
        if !question.isSelected {
            question.tap()
        }
        let send = element("messaging-send", in: app)
        XCTAssertTrue(send.waitForExistence(timeout: 3))
        XCTAssertTrue(send.isEnabled)
        send.tap()
    }

    @MainActor
    private func selectNativeTab(_ label: String, in app: XCUIApplication) {
        let direct = app.tabBars.buttons[label]
        if direct.waitForExistence(timeout: 5) {
            if direct.isSelected { return }
            if waitUntil(timeout: 15, condition: { direct.isHittable }) {
                direct.tap()
                return
            }
        }

        let more = app.tabBars.buttons["More"]
        XCTAssertTrue(more.waitForExistence(timeout: 2))
        more.tap()
        let destination = app.staticTexts[label].firstMatch
        XCTAssertTrue(destination.waitForExistence(timeout: 3))
        destination.tap()
    }

    @MainActor
    private func assertExactLegacyTabs(in app: XCUIApplication) {
        XCTAssertTrue(app.tabBars.buttons["Home"].waitForExistence(timeout: 12))
        let labels = Set(app.tabBars.buttons.allElementsBoundByIndex.map(\.label))
        XCTAssertEqual(labels, Set(["Home", "Today", "Tasks", "Alerts", "More"]))
        XCTAssertEqual(app.tabBars.buttons.count, 5)
        XCTAssertFalse(app.tabBars.buttons["Agents"].exists)
        XCTAssertTrue(app.tabBars.buttons["Tasks"].exists)
    }

    @MainActor
    private func assertMessageIsVisible(
        conversationID: String,
        sequence: Int,
        in app: XCUIApplication
    ) {
        let message = element("messaging-message-\(conversationID)-\(sequence)", in: app)
        XCTAssertTrue(message.waitForExistence(timeout: 10))
        let visibleFrame = message.frame.intersection(app.frame)
        XCTAssertFalse(
            visibleFrame.isNull || visibleFrame.isEmpty,
            "The deep-linked message exists but was not focused into the visible thread."
        )
    }

    @MainActor
    private func element(_ identifier: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    @MainActor
    private func exactText(_ label: String, in app: XCUIApplication) -> XCUIElementQuery {
        app.staticTexts.matching(NSPredicate(format: "label == %@", label))
    }

    @MainActor
    private func messagesFrom(_ agentID: String, in app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(
                format: "identifier BEGINSWITH %@",
                "messaging-message-from-\(agentID)-"
            )
        )
    }

    @MainActor
    private func loadedMessageCount(
        conversationID: String,
        in app: XCUIApplication
    ) throws -> Int {
        let thread = element("messaging-thread-\(conversationID)", in: app)
        XCTAssertTrue(thread.waitForExistence(timeout: 5))
        let description = String(describing: thread.value ?? thread.label)
            .replacingOccurrences(of: ",", with: "")
        let numericTokens = description.split(whereSeparator: { !$0.isNumber })
        guard let token = numericTokens.first, let count = Int(token) else {
            XCTFail("The thread did not expose its loaded logical-message count.")
            throw NSError(domain: "AgentMessagingUITests", code: 2)
        }
        return count
    }

    @MainActor
    private func latestWireTarget(
        from agentID: String,
        in app: XCUIApplication
    ) throws -> (conversationID: String, sequence: Int) {
        let targets = messagesFrom(agentID, in: app).allElementsBoundByIndex.compactMap {
            try? wireTarget(fromSenderMarker: $0, agentID: agentID)
        }
        guard let latest = targets.max(by: { lhs, rhs in
            lhs.sequence < rhs.sequence
        }) else {
            XCTFail("The latest canonical echo message was not materialized in the thread.")
            throw NSError(domain: "AgentMessagingUITests", code: 3)
        }
        return latest
    }

    @MainActor
    private func pendingOutboxRows(in app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "messaging-outbox-")
        )
    }

    @MainActor
    private func queuedOutboxRows(in app: XCUIApplication) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "messaging-outbox-queued-")
        )
    }

    @MainActor
    private func waitUntil(timeout: TimeInterval, condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        } while Date() < deadline
        return condition()
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

    private func conversationURL(conversationID: String, sequence: Int) throws -> URL {
        try XCTUnwrap(
            URL(string: "brunn://conversation/\(conversationID)?seq=\(sequence)")
        )
    }

    @MainActor
    private func wireTarget(
        fromSenderMarker marker: XCUIElement,
        agentID: String
    ) throws -> (conversationID: String, sequence: Int) {
        let prefix = "messaging-message-from-\(agentID)-"
        guard marker.identifier.hasPrefix(prefix) else {
            XCTFail("The echo sender marker did not carry a durable message identity.")
            throw NSError(domain: "AgentMessagingUITests", code: 1)
        }
        let suffix = marker.identifier.dropFirst(prefix.count)
        guard let separator = suffix.lastIndex(of: "-"),
              separator != suffix.startIndex,
              let sequence = Int(suffix[suffix.index(after: separator)...]),
              Self.isCanonicalUUIDv7(String(suffix[..<separator])),
              sequence > 0
        else {
            XCTFail("The echo sender marker did not carry a durable message identity.")
            throw NSError(domain: "AgentMessagingUITests", code: 1)
        }
        return (String(suffix[..<separator]), sequence)
    }

    private static func isCanonicalUUIDv7(_ value: String) -> Bool {
        value.range(
            of: #"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"#,
            options: .regularExpression
        ) != nil
    }

    private static func isOpaquePrincipalID(_ value: String) -> Bool {
        value.range(
            of: #"^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$"#,
            options: .regularExpression
        ) != nil
    }
}
