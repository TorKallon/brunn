@testable import Brunn
import Foundation
import UIKit
import XCTest

final class AgentMessagingTransportTests: XCTestCase {
    private let conversationID = "019f8800-0000-7000-8000-000000000001"

    func testConversationRouteAndNotificationTargetFailClosed() throws {
        let valid = try XCTUnwrap(URL(
            string: "brunn://conversation/\(conversationID)?seq=17"
        ))
        XCTAssertEqual(
            AppRoute(url: valid),
            .conversation(conversationID: conversationID, sequence: 17)
        )

        for raw in [
            "brunn://conversation/\(conversationID)",
            "brunn://conversation/\(conversationID)?seq=0",
            "brunn://conversation/\(conversationID)?seq=-1",
            "brunn://conversation/\(conversationID)?seq=word",
            "brunn://conversation/\(conversationID)?seq=17&extra=1",
            "brunn://conversation/\(conversationID.uppercased())?seq=17",
            "brunn://conversation/019f8800-0000-4000-8000-000000000001?seq=17",
        ] {
            XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(string: raw))), raw)
        }

        let target = try JSONDecoder().decode(
            BrunnNotificationTarget.self,
            from: Data(#"{"type":"conversation","conversation_id":"019f8800-0000-7000-8000-000000000001","seq":17}"#.utf8)
        )
        XCTAssertEqual(target.type, .conversation)
        XCTAssertEqual(target.conversationID, conversationID)
        XCTAssertEqual(target.sequence, 17)
        XCTAssertEqual(
            target.appRoute,
            .conversation(conversationID: conversationID, sequence: 17)
        )

        let invalidTarget = try JSONDecoder().decode(
            BrunnNotificationTarget.self,
            from: Data(#"{"type":"conversation","conversation_id":"019f8800-0000-4000-8000-000000000001","seq":17}"#.utf8)
        )
        XCTAssertNil(invalidTarget.appRoute)
    }

    func testGenericConversationPushRoutesAndRequiresContentAvailableForPrefetch() {
        let notificationID = "11111111111111111111111111111111"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let route = "brunn://conversation/\(conversationID)?seq=17"
        let payload: [AnyHashable: Any] = [
            "schema": "brunn-push@v1",
            "notification_ref": "notification:\(notificationID)",
            "delivery_ref": "delivery:\(deliveryID)",
            "brunn_route": route,
            "aps": [
                "alert": [
                    "title": "Brunn",
                    "body": "A new agent message is available.",
                ],
                "content-available": 1,
            ],
        ]

        XCTAssertEqual(
            NotificationRouteParser.route(from: payload),
            .conversation(conversationID: conversationID, sequence: 17)
        )
        XCTAssertTrue(NotificationRouteParser.isMessagingPrefetch(payload))

        var missingContentAvailable = payload
        missingContentAvailable["aps"] = ["alert": ["title": "Brunn"]]
        XCTAssertEqual(
            NotificationRouteParser.route(from: missingContentAvailable),
            .conversation(conversationID: conversationID, sequence: 17)
        )
        XCTAssertFalse(NotificationRouteParser.isMessagingPrefetch(missingContentAvailable))

        var malformedRoute = payload
        malformedRoute["brunn_route"] =
            "brunn://conversation/019f8800-0000-4000-8000-000000000001?seq=17"
        XCTAssertNil(NotificationRouteParser.route(from: malformedRoute))
        XCTAssertFalse(NotificationRouteParser.isMessagingPrefetch(malformedRoute))
    }

    @MainActor
    func testBackgroundPrefetchPostsBeforeReportingNewData() async {
        let posted = expectation(description: "prefetch posted")
        let completed = expectation(description: "background completion")
        let token = NotificationCenter.default.addObserver(
            forName: .brunnMessagingPrefetch,
            object: nil,
            queue: .main
        ) { event in
            posted.fulfill()
            let request = event.object as? MessagingBackgroundPrefetch
            request?.finish(.newData)
        }
        defer { NotificationCenter.default.removeObserver(token) }

        NotificationDelegateHandoff.finishBackgroundFetch(
            shouldPrefetch: true
        ) { result in
            XCTAssertEqual(result, .newData)
            completed.fulfill()
        }

        await fulfillment(of: [posted, completed], timeout: 1, enforceOrder: true)
    }

    @MainActor
    func testForegroundStatusRefreshDisablesMessagingAndLeavesAgentsTab() async throws {
        let fixture = try await makeMessagingModel(
            statusScript: MessagingStatusScript([true, false]),
            sync: { _ in Self.emptySync() }
        )
        defer { fixture.cleanup() }

        XCTAssertTrue(fixture.model.messagingEnabled)
        fixture.model.focusMessagingConversation(conversationID, sequence: 17)
        XCTAssertEqual(fixture.model.selectedTab, .agents)

        let outcome = await fixture.model.refreshMessaging(.foreground)

        XCTAssertEqual(outcome, .noData)
        XCTAssertFalse(fixture.model.messagingEnabled)
        XCTAssertEqual(fixture.model.selectedTab, .dashboard)
        XCTAssertNil(fixture.model.focusedMessagingConversationID)
        XCTAssertNil(fixture.model.focusedMessagingSequence)
    }

    @MainActor
    func testBackgroundRefreshWithNoChangesReportsNoData() async throws {
        let fixture = try await makeMessagingModel(
            statusScript: MessagingStatusScript([true]),
            sync: { _ in Self.emptySync() }
        )
        defer { fixture.cleanup() }

        let outcome = await fixture.model.refreshMessaging(.notificationPush)

        XCTAssertEqual(outcome, .noData)
        XCTAssertTrue(fixture.model.messagingEnabled)
    }

    @MainActor
    func testBackgroundRefreshTransportFailureReportsFailed() async throws {
        let fixture = try await makeMessagingModel(
            statusScript: MessagingStatusScript([true]),
            sync: { _ in throw URLError(.notConnectedToInternet) }
        )
        defer { fixture.cleanup() }

        let outcome = await fixture.model.refreshMessaging(.notificationPush)

        XCTAssertEqual(outcome, .failed)
        XCTAssertTrue(fixture.model.messagingEnabled)
        XCTAssertTrue(fixture.model.messagingMessage?.contains("could not refresh") == true)
    }

    @MainActor
    func testBackgroundRefreshWithPopulatedDeltaReportsNewData() async throws {
        let response = Self.populatedSync(conversationID: conversationID)
        let fixture = try await makeMessagingModel(
            statusScript: MessagingStatusScript([true]),
            sync: { _ in response }
        )
        defer { fixture.cleanup() }

        let outcome = await fixture.model.refreshMessaging(.notificationPush)

        XCTAssertEqual(outcome, .newData)
        XCTAssertEqual(fixture.model.messagingController?.conversations.count, 1)
        XCTAssertEqual(
            try fixture.model.messagingController?.messages(conversationID: conversationID)
                .map(\.bodyMarkdown),
            ["A fresh reply"]
        )
    }

    func testMessagingReadsUseCookieAndMutationsUseBearerWithoutCookies() async throws {
        let host = "messaging-transport-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "owner-session-secret",
            .secure: "TRUE",
        ]))
        let csrfCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_csrf",
            .value: "owner-csrf-secret",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        cookieStorage.setCookie(csrfCookie)
        defer {
            cookieStorage.deleteCookie(sessionCookie)
            cookieStorage.deleteCookie(csrfCookie)
            MessagingRequestURLProtocol.handler = nil
        }

        let recorder = MessagingRequestRecorder()
        let conversationID = conversationID
        let conversationEnvelope = conversationEnvelope
        let sendEnvelope = sendEnvelope
        MessagingRequestURLProtocol.handler = { request in
            recorder.append(request)
            switch (request.httpMethod, request.url?.path) {
            case ("GET", "/api/v1/status"):
                return .init(json: #"{"status":"ready","build_revision":"test","feature_flags":{"messaging_enabled":true}}"#)
            case ("GET", "/api/v1/workspace/messaging/sync"):
                return .init(json: #"{"status":"complete","data":{"status":"complete","cursor":3,"has_more":false,"messages":[],"conversations":[],"presence":[],"unread":{},"as_of":"2026-08-27T15:00:00Z"}}"#)
            case ("GET", "/api/v1/workspace/messaging/agents"):
                return .init(json: #"{"status":"complete","data":{"agents":[],"as_of":"2026-08-27T15:00:00Z"}}"#)
            case ("POST", "/api/v1/workspace/messaging/conversations"):
                return .init(json: conversationEnvelope)
            case ("POST", "/api/v1/workspace/messaging/conversations/\(conversationID)/messages"):
                return .init(json: sendEnvelope)
            case ("POST", "/api/v1/workspace/messaging/conversations/\(conversationID)/read"):
                return .init(json: #"{"status":"committed","data":{"conversation_id":"019f8800-0000-7000-8000-000000000001","last_read_seq":1,"cursor":4,"duplicate":false}}"#)
            case ("PUT", "/api/v1/workspace/messaging/agents/owner/credential"):
                return .init(json: #"{"status":"committed","data":{"agent_id":"owner","credential_id":"11111111-1111-4111-8111-111111111111","bound":true}}"#)
            default:
                return .init(
                    statusCode: 404,
                    json: #"{"error":{"code":"unexpected_test_request","message":"unexpected"}}"#
                )
            }
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MessagingRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )
        let exactSend = Data(#"{"body_md":"hello","client_key":"01K3N0RTH00000000000000000","expects_reply":false,"kind":"text","refs":[]}"#.utf8)

        let status = try await api.messagingStatus()
        let sync = try await api.messagingSync(MessagingSyncRequest(
            cursor: 2,
            waitSeconds: 25,
            conversationID: conversationID,
            afterSequence: 7,
            limit: 50
        ))
        _ = try await api.messagingAgents()
        _ = try await api.createMessagingConversation(
            MessagingCreateConversationRequest(participants: ["echo"]),
            bearerToken: "narrow-device-token"
        )
        _ = try await api.sendMessagingMessage(
            conversationID: conversationID,
            exactRequestData: exactSend,
            bearerToken: "narrow-device-token"
        )
        _ = try await api.markMessagingRead(
            conversationID: conversationID,
            lastReadSeq: 1,
            bearerToken: "narrow-device-token"
        )
        do {
            _ = try await api.bindMessagingCredential(
                agentID: "owner",
                credentialReference: "credential:AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"
            )
            XCTFail("Non-canonical credential references must fail before transport")
        } catch {
            XCTAssertEqual(error as? BrunnAPIError, .invalidConfiguration)
        }
        _ = try await api.bindMessagingCredential(
            agentID: "owner",
            credentialReference: "credential:11111111-1111-4111-8111-111111111111"
        )

        XCTAssertEqual(status.featureFlags?.messagingEnabled, true)
        XCTAssertEqual(sync.cursor, 3)
        let requests = recorder.snapshot()
        XCTAssertEqual(requests.count, 7)
        XCTAssertEqual(
            requests.map(\.httpMethod),
            ["GET", "GET", "GET", "POST", "POST", "POST", "PUT"]
        )

        for request in requests.prefix(3) {
            XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
            XCTAssertTrue(request.httpShouldHandleCookies)
        }
        for request in requests[3 ..< 6] {
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer narrow-device-token"
            )
            XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
            XCTAssertNil(request.value(forHTTPHeaderField: "X-CSRF-Token"))
        }

        let syncItems = try XCTUnwrap(URLComponents(
            url: try XCTUnwrap(requests[1].url),
            resolvingAgainstBaseURL: false
        )?.queryItems)
        let syncQuery: [String: String?] = Dictionary(
            uniqueKeysWithValues: syncItems.map { ($0.name, $0.value) }
        )
        XCTAssertEqual(syncQuery, [
            "after_seq": "7",
            "conversation_id": conversationID,
            "cursor": "2",
            "limit": "50",
            "wait": "25",
        ])
        let createBody = try XCTUnwrap(requests[3].httpBody)
        let create = try XCTUnwrap(
            JSONSerialization.jsonObject(with: createBody) as? [String: Any]
        )
        XCTAssertEqual(create["participants"] as? [String], ["echo"])
        XCTAssertNil(create["from"])
        XCTAssertEqual(requests[4].httpBody, exactSend)
        let readBody = try XCTUnwrap(requests[5].httpBody)
        let read = try XCTUnwrap(
            JSONSerialization.jsonObject(with: readBody) as? [String: Any]
        )
        XCTAssertEqual(read["last_read_seq"] as? Int, 1)
        let bindingRequest = requests[6]
        XCTAssertNil(bindingRequest.value(forHTTPHeaderField: "Authorization"))
        XCTAssertTrue(bindingRequest.httpShouldHandleCookies)
        XCTAssertEqual(
            Set((cookieStorage.cookies(for: baseURL) ?? []).map(\.name)),
            Set(["brunn_session", "brunn_csrf"])
        )
        XCTAssertEqual(
            bindingRequest.value(forHTTPHeaderField: "X-CSRF-Token"),
            "owner-csrf-secret"
        )
        let bindingBody = try XCTUnwrap(bindingRequest.httpBody)
        let binding = try XCTUnwrap(
            JSONSerialization.jsonObject(with: bindingBody) as? [String: Any]
        )
        XCTAssertEqual(
            binding["credential_id"] as? String,
            "11111111-1111-4111-8111-111111111111"
        )
    }

    private var conversationEnvelope: String {
        #"{"status":"committed","data":{"conversation_id":"019f8800-0000-7000-8000-000000000001","conversation":{"conversation_id":"019f8800-0000-7000-8000-000000000001","conversation_kind":"direct","subject":null,"status":"open","participants":[{"agent_id":"owner","role":"participant"},{"agent_id":"echo","role":"participant"}],"last_seq":0,"last_message_at":null,"last_read_seq":0,"unread_count":0,"needs_human":false,"latest_sync_cursor":1},"duplicate":false}}"#
    }

    private var sendEnvelope: String {
        #"{"status":"committed","data":{"conversation_id":"019f8800-0000-7000-8000-000000000001","seq":1,"message":{"conversation_id":"019f8800-0000-7000-8000-000000000001","seq":1,"message_id":"019f8800-0000-7000-8000-000000000002","from_agent_id":"owner","client_key":"01K3N0RTH00000000000000000","kind":"text","body_md":"hello","refs":[],"expects_reply":false,"sync_cursor":2,"created_at":"2026-08-27T15:00:00Z"},"duplicate":false}}"#
    }

    @MainActor
    private func makeMessagingModel(
        statusScript: MessagingStatusScript,
        sync: @escaping @Sendable (MessagingSyncRequest) async throws -> MessagingSyncResponse
    ) async throws -> MessagingModelFixture {
        let host = "messaging-model-\(UUID().uuidString.lowercased()).brunn.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared
        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "brunn_session",
            .value: "model-session-secret",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)

        MessagingRequestURLProtocol.handler = { request in
            if request.httpMethod == "GET", request.url?.path == "/api/v1/status" {
                return statusScript.nextResponse()
            }
            return .init(
                statusCode: 503,
                json: #"{"error":{"code":"offline_test_fixture","message":"offline"}}"#
            )
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [MessagingRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = BrunnAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )
        let rootURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("AgentMessagingModelTests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let store = try MessagingStore(isStoredInMemoryOnly: true)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: sync,
                send: { _, _ in throw MessagingTransportError.operationNotConfigured }
            )
        )
        let model = AppModel(
            api: api,
            credentialStore: MessagingEmptyCredentialStore(),
            briefingCache: BriefingCache(
                fileURL: rootURL.appendingPathComponent("briefing.json")
            ),
            taskSurfaceCache: TaskSurfaceCache(
                fileURL: rootURL.appendingPathComponent("tasks.json")
            ),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                MeData(
                    user: UserSummary(id: "user:messaging-tests", displayName: "Owner"),
                    capabilities: ["query", "read", "status"],
                    readOnly: true
                )
            },
            dashboardLoader: { _, _ in throw URLError(.notConnectedToInternet) },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [], unreadCount: 0)
            },
            messagingController: controller
        )

        await model.bootstrap()
        await Task.yield()
        return MessagingModelFixture(
            model: model,
            sessionCookie: sessionCookie,
            cookieStorage: cookieStorage,
            rootURL: rootURL
        )
    }

    private static func emptySync() -> MessagingSyncResponse {
        MessagingSyncResponse(
            status: "complete",
            cursor: 0,
            hasMore: false,
            messages: [],
            conversations: [],
            presence: [],
            unread: [:],
            asOf: "2026-08-27T15:00:00Z"
        )
    }

    private static func populatedSync(conversationID: String) -> MessagingSyncResponse {
        MessagingSyncResponse(
            status: "complete",
            cursor: 1,
            hasMore: false,
            messages: [MessagingMessage(
                conversationID: conversationID,
                sequence: 1,
                messageID: "019f8800-0000-7000-8000-000000000002",
                fromAgentID: "echo",
                clientKey: nil,
                kind: "text",
                bodyMarkdown: "A fresh reply",
                refs: [],
                expectsReply: false,
                syncCursor: 1,
                createdAt: "2026-08-27T15:00:00Z"
            )],
            conversations: [MessagingConversation(
                conversationID: conversationID,
                conversationKind: "direct",
                subject: nil,
                status: "open",
                participants: [
                    MessagingParticipant(agentID: "owner", role: "participant"),
                    MessagingParticipant(agentID: "echo", role: "participant"),
                ],
                lastSeq: 1,
                lastMessageAt: "2026-08-27T15:00:00Z",
                lastReadSeq: 0,
                unreadCount: 1,
                needsHuman: false,
                latestSyncCursor: 1
            )],
            presence: [],
            unread: [conversationID: 1],
            asOf: "2026-08-27T15:00:00Z"
        )
    }
}

@MainActor
private struct MessagingModelFixture {
    let model: AppModel
    let sessionCookie: HTTPCookie
    let cookieStorage: HTTPCookieStorage
    let rootURL: URL

    func cleanup() {
        MessagingRequestURLProtocol.handler = nil
        cookieStorage.deleteCookie(sessionCookie)
        try? FileManager.default.removeItem(at: rootURL)
    }
}

@MainActor
private final class MessagingEmptyCredentialStore: CredentialStoring {
    func load() throws -> DeviceTaskCredential? { nil }
    func save(_: DeviceTaskCredential) throws {}
    func delete() throws {}
}

private final class MessagingStatusScript: @unchecked Sendable {
    private let lock = NSLock()
    private let values: [Bool]
    private var index = 0

    init(_ values: [Bool]) {
        precondition(!values.isEmpty)
        self.values = values
    }

    func nextResponse() -> MessagingStubbedHTTPResponse {
        lock.lock()
        defer { lock.unlock() }
        let value = values[min(index, values.count - 1)]
        index += 1
        return MessagingStubbedHTTPResponse(
            json: """
            {"status":"ready","build_revision":"test","feature_flags":{"messaging_enabled":\(value)}}
            """
        )
    }
}

private struct MessagingStubbedHTTPResponse {
    let statusCode: Int
    let json: String

    init(statusCode: Int = 200, json: String) {
        self.statusCode = statusCode
        self.json = json
    }
}

private final class MessagingRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [URLRequest] = []

    func append(_ request: URLRequest) {
        lock.lock()
        requests.append(request)
        lock.unlock()
    }

    func snapshot() -> [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }
}

private final class MessagingRequestURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> MessagingStubbedHTTPResponse)?

    override class func canInit(with _: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        var captured = request
        if captured.httpBody == nil, let stream = captured.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var data = Data()
            let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4_096)
            defer { buffer.deallocate() }
            while stream.hasBytesAvailable {
                let count = stream.read(buffer, maxLength: 4_096)
                guard count > 0 else { break }
                data.append(buffer, count: count)
            }
            captured.httpBody = data
        }
        let stub = handler(captured)
        guard let response = HTTPURLResponse(
            url: url,
            statusCode: stub.statusCode,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        ) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(stub.json.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
