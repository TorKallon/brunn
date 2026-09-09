@testable import BrunnCore
import XCTest

final class DocumentContractTests: XCTestCase {
    func testSharedContractRoundTripsAndRejectsMalformedLinks() throws {
        struct Contract: Decodable {
            struct Valid: Decodable { let slug: String; let version: Int64?; let url: String }
            let valid: [Valid]
            let invalid: [String]
        }
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let contract = try JSONDecoder().decode(Contract.self,
            from: Data(contentsOf: root.appendingPathComponent("contracts/document-links.json")))
        for example in contract.valid {
            let link = try XCTUnwrap(DocumentLink(slug: example.slug, version: example.version))
            XCTAssertEqual(link.url.absoluteString, example.url)
            XCTAssertEqual(DocumentLink(url: link.url), link)
            XCTAssertEqual(AppRoute(url: link.url), .document(link))
            XCTAssertEqual(link.latest.version, nil)
        }
        for invalid in contract.invalid {
            if let url = URL(string: invalid) {
                XCTAssertNil(DocumentLink(url: url), invalid)
                XCTAssertNil(AppRoute(url: url), invalid)
            }
        }
        let maximum = try XCTUnwrap(DocumentLink(slug: String(repeating: "a", count: 80), version: Int64.max))
        XCTAssertEqual(DocumentLink(url: maximum.url), maximum)
        XCTAssertNil(DocumentLink(slug: String(repeating: "a", count: 81)))
        XCTAssertNil(DocumentLink(slug: "trip-plan", version: 0))
        XCTAssertNil(DocumentLink(slug: "trip-plan", version: -1))
        XCTAssertEqual(DocumentLink(url: URL(string: "BRUNN://DOCUMENT/trip-plan")!)?.slug, "trip-plan")
    }

    func testEveryExistingRouteKeepsItsMeaning() throws {
        let task = "01a08731-b88f-7711-93e2-5b3e1b33baa3"
        let notification = String(repeating: "a", count: 32)
        let delivery = String(repeating: "b", count: 32)
        let routes: [(String, AppRoute)] = [
            ("brunn://today", .today),
            ("brunn://review", .review),
            ("brunn://dreams", .review),
            ("brunn://briefing/2026-09-09/morning", .briefing(date: "2026-09-09", edition: "morning", itemID: nil)),
            ("brunn://briefing/2026-09-09/morning?item=story", .briefing(date: "2026-09-09", edition: "morning", itemID: "story")),
            ("brunn://task/\(task)", .task(reference: task)),
            ("brunn://conversation/\(task)?seq=2", .conversation(conversationID: task, sequence: 2)),
            ("brunn://notification/\(notification)?delivery=\(delivery)",
                .notification(notificationRef: "notification:\(notification)", deliveryRef: "delivery:\(delivery)")),
            ("brunn://notification/\(notification)", .notification(notificationRef: "notification:\(notification)", deliveryRef: nil)),
        ]
        for (url, expected) in routes {
            XCTAssertEqual(AppRoute(url: try XCTUnwrap(URL(string: url))), expected, url)
        }
    }

    func testDocumentEnvelopeDecodingAndExactRevisionSemantics() throws {
        let document = try JSONDecoder().decode(WorkspaceEnvelope<PublishedDocument>.self,
            from: Data(Self.fixture.utf8)).data
        XCTAssertEqual(document.bodyMD, "## Plan\n\nFull published body.")
        XCTAssertEqual(document.sources[1].entryRef, "entry:private-source")
        XCTAssertEqual(document.appURL, "brunn://document/trip-plan")
        XCTAssertTrue(document.matches(DocumentLink(slug: "trip-plan")!))
        XCTAssertTrue(document.matches(DocumentLink(slug: "trip-plan", version: 2)!))
        XCTAssertFalse(document.matches(DocumentLink(slug: "trip-plan", version: 1)!))
        XCTAssertFalse(document.matches(DocumentLink(slug: "other-plan")!))
        let historical = Self.fixture.replacingOccurrences(of: "\"version\":2,\"current_version\":2",
            with: "\"version\":1,\"current_version\":2")
        let pinned = try JSONDecoder().decode(WorkspaceEnvelope<PublishedDocument>.self, from: Data(historical.utf8)).data
        XCTAssertTrue(pinned.matches(DocumentLink(slug: "trip-plan", version: 1)!))
        XCTAssertFalse(pinned.matches(DocumentLink(slug: "trip-plan")!))
    }

    func testAuthenticatedRequestsAreBodylessUncachedAndNeverFallback() async throws {
        let harness = DocumentAPIHarness()
        defer { harness.close() }
        DocumentURLProtocol.handler = { request in
            XCTAssertEqual(request.httpMethod, "GET")
            XCTAssertNil(request.httpBody)
            XCTAssertEqual(request.url?.path, "/api/v1/workspace/documents/trip-plan")
            XCTAssertFalse(request.httpShouldHandleCookies)
            XCTAssertTrue(request.value(forHTTPHeaderField: "Cookie")?.contains("owner-session") == true)
            XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
            XCTAssertEqual(request.cachePolicy, .reloadIgnoringLocalCacheData)
            return (200, Data(Self.fixture.utf8))
        }
        let latest = try await harness.api.document(DocumentLink(slug: "trip-plan")!)
        XCTAssertEqual(latest.version, 2)
        DocumentURLProtocol.handler = { request in
            XCTAssertEqual(request.url?.query, "version=1")
            return (404, Data(#"{"error":{"code":"document_not_found","message":"Unavailable"}}"#.utf8))
        }
        do {
            _ = try await harness.api.document(DocumentLink(slug: "trip-plan", version: 1)!)
            XCTFail("A missing pinned revision must not return latest")
        } catch let error as BrunnAPIError {
            XCTAssertEqual(error, .server(status: 404, code: "document_not_found", message: "Unavailable"))
        }
        DocumentURLProtocol.handler = { _ in (200, Data(Self.fixture.utf8)) }
        do {
            _ = try await harness.api.document(DocumentLink(slug: "trip-plan", version: 1)!)
            XCTFail("A mismatched server revision must be rejected")
        } catch let error as BrunnAPIError { XCTAssertEqual(error, .invalidResponse) }
    }

    func testStableLinkReadsAgainAfterRepublishingAndOldServersRemainDecodable() async throws {
        let harness = DocumentAPIHarness()
        defer { harness.close() }
        let oldPayload = Self.fixture.replacingOccurrences(of: "\"app_url\":\"brunn://document/trip-plan\",", with: "")
            .replacingOccurrences(of: "\"app_version_url\":\"brunn://document/trip-plan?version=2\",", with: "")
        DocumentURLProtocol.handler = { request in
            XCTAssertNil(request.url?.query)
            return (200, Data(oldPayload.utf8))
        }
        let link = DocumentLink(slug: "trip-plan")!
        let first = try await harness.api.document(link)
        XCTAssertEqual(first.version, 2)
        XCTAssertNil(first.appURL)
        DocumentURLProtocol.handler = { _ in
            (200, Data(Self.fixture.replacingOccurrences(of: ":2", with: ":3").utf8))
        }
        let next = try await harness.api.document(link)
        XCTAssertEqual(next.version, 3)
    }

    func testNoSessionDoesNotIssueAReadAndUnauthorizedIsNotReplaced() async throws {
        let harness = DocumentAPIHarness()
        defer { harness.close() }
        DocumentURLProtocol.handler = { _ in (401, Data(#"{"error":{"code":"unauthorized","message":"Sign in"}}"#.utf8)) }
        do {
            _ = try await harness.api.document(DocumentLink(slug: "trip-plan")!)
            XCTFail("Authentication must be required")
        } catch let error as BrunnAPIError { XCTAssertTrue(error.isUnauthorized) }
        harness.cookies.deleteCookie(harness.cookie)
        DocumentURLProtocol.handler = { _ in
            XCTFail("No read should be sent without the session")
            return (200, Data(Self.fixture.utf8))
        }
        do {
            _ = try await harness.api.document(DocumentLink(slug: "trip-plan")!)
            XCTFail("Authentication must be required")
        } catch let error as BrunnAPIError { XCTAssertEqual(error, .notConnected) }
    }

    func testResponseCannotCrossAnAccountSessionChange() async throws {
        let harness = DocumentAPIHarness()
        defer { harness.close() }
        let cookies = harness.cookies
        let oldCookie = harness.cookie
        DocumentURLProtocol.handler = { request in
            XCTAssertTrue(request.value(forHTTPHeaderField: "Cookie")?.contains("owner-session") == true)
            cookies.deleteCookie(oldCookie)
            return (200, Data(Self.fixture.utf8))
        }
        do {
            _ = try await harness.api.document(DocumentLink(slug: "trip-plan")!)
            XCTFail("An old session's result must not survive account switching")
        } catch let error as BrunnAPIError { XCTAssertEqual(error, .notConnected) }
    }

    static let fixture = #"""
    {"status":"complete","data":{
    "slug":"trip-plan","title":"Trip plan","summary":"A complete handoff",
    "body_md":"## Plan\n\nFull published body.",
    "sources":[{"label":"External","url":"https://example.com/source"},{"label":"Private","entry_ref":"entry:private-source"}],
    "version":2,"current_version":2,"published_at":"2026-09-09T12:00:00Z","updated_at":"2026-09-09T13:00:00Z",
    "app_url":"brunn://document/trip-plan","app_version_url":"brunn://document/trip-plan?version=2",
    "url":"https://brunn.ai/documents/trip-plan","version_url":"https://brunn.ai/documents/trip-plan?version=2",
    "versions":[{"version":1,"created_at":"2026-09-09T12:00:00Z","version_url":"https://brunn.ai/documents/trip-plan?version=1"},
    {"version":2,"created_at":"2026-09-09T13:00:00Z","version_url":"https://brunn.ai/documents/trip-plan?version=2"}]
    }}
    """#
}

private struct DocumentAPIHarness {
    let api: BrunnAPI
    let cookies: HTTPCookieStorage
    let cookie: HTTPCookie
    let session: URLSession

    init() {
        let host = "document-\(UUID().uuidString.lowercased()).brunn.test"
        let url = URL(string: "https://\(host)/api/v1")!
        cookies = .shared
        cookie = HTTPCookie(properties: [.domain: host, .path: "/", .name: "brunn_session", .value: "owner-session", .secure: "TRUE"])!
        cookies.setCookie(cookie)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [DocumentURLProtocol.self]
        config.httpCookieStorage = cookies
        session = URLSession(configuration: config)
        api = BrunnAPI(configuration: .init(baseURL: url), session: session, cookieStorage: cookies)
    }

    func close() {
        session.invalidateAndCancel()
        cookies.deleteCookie(cookie)
        DocumentURLProtocol.handler = nil
    }
}

private final class DocumentURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (Int, Data))?
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        guard let handler = Self.handler else { return }
        let (status, data) = handler(request)
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}
