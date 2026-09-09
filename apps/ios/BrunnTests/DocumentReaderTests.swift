@testable import Brunn
import XCTest

@MainActor
final class DocumentReaderTests: XCTestCase {
    func testColdLaunchReplaysTheExactPinnedDestinationAfterValidation() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        let link = DocumentLink(slug: "first-plan", version: 1)!
        await harness.model.handle(.document(link))
        XCTAssertEqual(harness.model.pendingDocumentLink, link)
        XCTAssertNil(harness.model.documentRequest)
        let bootstrap = Task { await harness.model.bootstrap() }
        await harness.gate.waitFor(link)
        XCTAssertEqual(harness.model.documentRequest, link)
        XCTAssertEqual(harness.model.documentState, .loading)
        await harness.gate.finish(link)
        await bootstrap.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(link)))
        XCTAssertNil(harness.model.pendingDocumentLink)
    }

    func testSignedOutLaunchKeepsDestinationThroughSignIn() async throws {
        let harness = ReaderHarness(storedSession: false)
        defer { harness.close() }
        let link = DocumentLink(slug: "first-plan", version: 1)!
        await harness.model.handle(.document(link))
        await harness.model.bootstrap()
        XCTAssertEqual(harness.model.phase, .connectionRequired)
        XCTAssertEqual(harness.model.pendingDocumentLink, link)
        let connect = Task { await harness.model.connect(email: "reader@example.test", password: "test-only") }
        await harness.gate.waitFor(link)
        await harness.gate.finish(link)
        await connect.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(link)))
    }

    func testOfflineCachedLaunchDoesNotConsumePendingDocumentBeforeSignIn() async throws {
        let harness = ReaderHarness(failBootstrap: true)
        defer { harness.close() }
        try await BriefingCache(fileURL: harness.directory.appendingPathComponent("briefing.json"))
            .save(SampleData.briefing)
        let link = DocumentLink(slug: "first-plan", version: 1)!
        await harness.model.handle(.document(link))
        await harness.model.bootstrap()
        XCTAssertEqual(harness.model.phase, .ready)
        XCTAssertFalse(harness.model.connectionValidated)
        XCTAssertEqual(harness.model.documentRequest, link)
        XCTAssertEqual(harness.model.documentState, .authenticationRequired)
        XCTAssertEqual(harness.model.pendingDocumentLink, link)
        harness.model.signInForDocument()
        let connect = Task { await harness.model.connect(email: "reader@example.test", password: "test-only") }
        await harness.gate.waitFor(link)
        await harness.gate.finish(link)
        await connect.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(link)))
    }

    func testNewWarmRequestCancelsAndFencesAnOlderUncooperativeResponse() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        let first = DocumentLink(slug: "first-plan")!
        let second = DocumentLink(slug: "second-plan", version: 1)!
        let old = Task { await harness.model.handle(.document(first)) }
        await harness.gate.waitFor(first)
        let new = Task { await harness.model.handle(.document(second)) }
        await harness.gate.waitFor(second)
        XCTAssertEqual(harness.model.documentRequest, second)
        XCTAssertEqual(harness.model.documentState, .loading)
        await harness.gate.finish(second)
        await new.value
        await harness.gate.finish(first)
        await old.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(second)))
        let cancelled = await harness.gate.cancelled
        XCTAssertTrue(cancelled.contains(first))
    }

    func testDismissAndOtherRoutesCancelLoadsWithoutReopeningReader() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        for dismiss in [true, false] {
            let link = DocumentLink(slug: dismiss ? "dismiss-plan" : "briefing-plan")!
            let loading = Task { await harness.model.handle(.document(link)) }
            await harness.gate.waitFor(link)
            if dismiss { harness.model.dismissDocument() }
            else { await harness.model.handle(.today) }
            await harness.gate.finish(link)
            await loading.value
            XCTAssertNil(harness.model.documentRequest)
            XCTAssertEqual(harness.model.documentState, .loading)
            XCTAssertNil(harness.model.pendingDocumentLink)
        }
        XCTAssertEqual(harness.model.selectedTab, .today)
    }

    func testCancellationIsRetryableAndDoesNotPaintContent() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        let link = DocumentLink(slug: "first-plan")!
        let loading = Task { await harness.model.handle(.document(link)) }
        await harness.gate.waitFor(link)
        loading.cancel()
        await harness.gate.finish(link)
        await loading.value
        XCTAssertEqual(harness.model.documentState, .failed("Loading was cancelled. Try again."))
        let retry = Task { await harness.model.retryDocument() }
        await harness.gate.waitFor(link)
        await harness.gate.finish(link)
        await retry.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(link)))
    }

    func testLogoutClearsProtectedContentAndPendingDestinationBeforeNetworkCompletes() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        let link = DocumentLink(slug: "first-plan")!
        let loading = Task { await harness.model.handle(.document(link)) }
        await harness.gate.waitFor(link)
        await harness.model.disconnect()
        XCTAssertNil(harness.model.documentRequest)
        XCTAssertNil(harness.model.pendingDocumentLink)
        await harness.gate.finish(link)
        await loading.value
        XCTAssertEqual(harness.model.phase, .connectionRequired)
        XCTAssertNil(harness.model.documentRequest)
        XCTAssertEqual(harness.model.documentState, .loading)
    }

    func testAccountSwitchCannotRestorePriorAccountsDocument() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        let link = DocumentLink(slug: "first-plan")!
        let loading = Task { await harness.model.handle(.document(link)) }
        await harness.gate.waitFor(link)
        await harness.model.connect(email: "other@example.test", password: "test-only")
        XCTAssertEqual(harness.model.user?.id, "user:other")
        XCTAssertNil(harness.model.documentRequest)
        await harness.gate.finish(link)
        await loading.value
        XCTAssertNil(harness.model.documentRequest)
        XCTAssertEqual(harness.model.documentState, .loading)
    }

    func testFailuresStayOnRequestedDocumentAndSignInReplaysIt() async throws {
        let harness = ReaderHarness()
        defer { harness.close() }
        await harness.model.bootstrap()
        let link = DocumentLink(slug: "first-plan", version: 1)!
        for error in [BrunnAPIError.server(status: 404, code: "document_not_found", message: "Unavailable"),
                      .server(status: 403, code: "forbidden", message: "Forbidden"),
                      .server(status: 503, code: "unavailable", message: "Network failure"),
                      .server(status: 401, code: "unauthorized", message: "Expired")] {
            let loading = Task { await harness.model.handle(.document(link)) }
            await harness.gate.waitFor(link)
            await harness.gate.fail(link, error: error)
            await loading.value
            XCTAssertEqual(harness.model.documentRequest, link)
            switch error {
            case .server(404, _, _), .server(403, _, _): XCTAssertEqual(harness.model.documentState, .unavailable)
            case .server(401, _, _): XCTAssertEqual(harness.model.documentState, .authenticationRequired)
            default:
                guard case .failed = harness.model.documentState else { return XCTFail("Retryable network failure") }
            }
        }
        XCTAssertEqual(harness.model.pendingDocumentLink, link)
        harness.model.signInForDocument()
        XCTAssertEqual(harness.model.phase, .connectionRequired)
        XCTAssertEqual(harness.model.pendingDocumentLink, link)
        let login = Task { await harness.model.connect(email: "reader@example.test", password: "test-only") }
        await harness.gate.waitFor(link)
        await harness.gate.finish(link)
        await login.value
        XCTAssertEqual(harness.model.documentState, .loaded(try readerFixture(link)))
    }

    func testDocumentLinksFilterUnsafeAndPrivateDestinations() {
        let markdown = """
        [Web](https://example.com/source) [Document](brunn://document/first-plan?version=1)
        [Briefing](brunn://briefing/2026-09-09/morning) [Task](brunn://task/01a08731-b88f-7711-93e2-5b3e1b33baa3)
        [Private](entry:01a08731-b88f-7711-93e2-5b3e1b33baa3) [Relative](../secrets.md)
        [File](file:///private/source) [JS](javascript:alert) [Credentials](https://owner:secret@example.com)
        [Unknown](brunn://raw/secret) [Wrong version](brunn://document/first-plan?version=01)
        [Entry reader](brunn-entry://open?target=secret) [[Private wiki reference]]
        """
        let text = SafeMarkdown.documentAttributedString(markdown)
        XCTAssertEqual(text.runs.compactMap(\.link).map(\.absoluteString), [
            "https://example.com/source", "brunn://document/first-plan?version=1",
            "brunn://briefing/2026-09-09/morning", "brunn://task/01a08731-b88f-7711-93e2-5b3e1b33baa3",
        ])
        XCTAssertTrue(String(text.characters).contains("Private"))
    }

    func testNativeMarkdownKeepsBlockStructureInlineFormattingAndEveryTableCell() throws {
        let document = try SampleData.document(DocumentLink(slug: "demo-handoff")!)
        let longTail = String(repeating: "\n\n### Extended handoff\n\nAll detailed source-backed prose stays readable, including [links](https://example.com/source).", count: 240)
        let nodes = DocumentMarkdownNode.parse(document.bodyMD + longTail + "\n\nLast line of the long handoff.")
        func flatten(_ nodes: [DocumentMarkdownNode]) -> [DocumentMarkdownNode] {
            nodes.flatMap { [$0] + flatten($0.children) }
        }
        let all = flatten(nodes)
        XCTAssertTrue(all.contains { $0.kind == .header(level: 2) })
        XCTAssertTrue(all.contains { $0.kind == .orderedList })
        XCTAssertTrue(all.contains { $0.kind == .unorderedList })
        XCTAssertTrue(all.contains { $0.kind == .blockQuote })
        XCTAssertTrue(all.contains { if case .codeBlock = $0.kind { return true }; return false })
        let table = try XCTUnwrap(all.first { if case .table = $0.kind { return true }; return false })
        XCTAssertEqual(table.children.count, 3)
        XCTAssertEqual(table.children.map { $0.children.count }, [3, 3, 3])
        XCTAssertTrue(all.contains { String($0.text.characters).contains("Final detail: the complete handoff is preserved.") })
        XCTAssertEqual(String(try XCTUnwrap(all.last).text.characters), "Last line of the long handoff.")
        XCTAssertTrue(all.flatMap { Array($0.text.runs) }.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true })
        XCTAssertTrue(all.flatMap { Array($0.text.runs) }.contains { $0.link?.absoluteString == "brunn://document/demo-handoff?version=1" })
    }
}

private actor ReaderGate {
    private var requests: [DocumentLink: CheckedContinuation<PublishedDocument, Error>] = [:]
    private(set) var cancelled: Set<DocumentLink> = []
    func read(_ link: DocumentLink) async throws -> PublishedDocument {
        let result = try await withCheckedThrowingContinuation { requests[link] = $0 }
        if Task.isCancelled { cancelled.insert(link) }
        return result
    }
    func waitFor(_ link: DocumentLink) async {
        for _ in 0 ..< 2_000 {
            if requests[link] != nil { return }
            try? await Task.sleep(for: .milliseconds(1))
        }
        XCTFail("Document request did not arrive: \(link)")
    }
    func finish(_ link: DocumentLink) {
        requests.removeValue(forKey: link)?.resume(with: Result { try readerFixture(link) })
    }
    func fail(_ link: DocumentLink, error: Error) { requests.removeValue(forKey: link)?.resume(throwing: error) }
}

private func readerFixture(_ link: DocumentLink) throws -> PublishedDocument {
    let demo = try SampleData.document(DocumentLink(slug: "demo-handoff", version: link.version)!)
    var json = try JSONSerialization.jsonObject(with: JSONEncoder().encode(demo)) as! [String: Any]
    json["slug"] = link.slug
    json["title"] = link.slug
    return try JSONDecoder().decode(PublishedDocument.self, from: JSONSerialization.data(withJSONObject: json))
}

@MainActor
private struct ReaderHarness {
    let model: AppModel
    let gate = ReaderGate()
    let session: URLSession
    let directory: URL

    init(storedSession: Bool = true, failBootstrap: Bool = false) {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ReaderBackgroundURLProtocol.self]
        session = URLSession(configuration: config)
        let api = BrunnAPI(configuration: .init(baseURL: URL(string: "https://reader.brunn.test/api/v1")!), session: session)
        let gate = gate
        model = AppModel(api: api, credentialStore: ReaderCredentialStore(),
            briefingCache: BriefingCache(fileURL: directory.appendingPathComponent("briefing.json")),
            taskSurfaceCache: TaskSurfaceCache(fileURL: directory.appendingPathComponent("tasks.json")),
            storedSessionChecker: { _ in storedSession },
            bootstrapIdentityLoader: { _ in
                if failBootstrap { throw URLError(.notConnectedToInternet) }
                return Self.identity("reader")
            },
            loginLoader: { _, email, _ in Self.identity(email.hasPrefix("other") ? "other" : "reader") },
            dashboardLoader: { _, _ in SampleData.dashboard },
            documentLoader: { _, link in try await gate.read(link) },
            notificationListLoader: { _, _ in NotificationListResponse(items: [], nextCursor: nil, unreadCount: 0) })
    }
    nonisolated static func identity(_ name: String) -> MeData {
        MeData(user: UserSummary(id: "user:\(name)", displayName: name), credentialID: "credential:reader", capabilities: ["read"], readOnly: true)
    }
    func close() {
        model.dismissDocument()
        session.invalidateAndCancel()
        try? FileManager.default.removeItem(at: directory)
    }
}

@MainActor
private final class ReaderCredentialStore: CredentialStoring {
    func load() throws -> DeviceTaskCredential? { nil }
    func save(_ credential: DeviceTaskCredential) throws {}
    func delete() throws {}
}

private final class ReaderBackgroundURLProtocol: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: 404, httpVersion: nil, headerFields: nil)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(#"{"error":{"code":"test_background_request","message":"Not part of document test"}}"#.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}
