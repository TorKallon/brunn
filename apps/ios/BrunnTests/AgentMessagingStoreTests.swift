@testable import Brunn
import Foundation
import XCTest

final class AgentMessagingStoreTests: XCTestCase {
    private let accountA = "user:account-a"
    private let fingerprintA = "sha256:" + String(repeating: "a", count: 64)
    private let fingerprintB = "sha256:" + String(repeating: "b", count: 64)
    private let conversationID = "018f89c8-3f08-7b85-9a4d-f26e269f672d"
    private let clientKey = "01K3N0RTH00000000000000000"

    @MainActor
    func testSchemaHasExactlyFourPersistentModels() throws {
        let store = try MessagingStore(isStoredInMemoryOnly: true)

        XCTAssertEqual(MessagingStore.persistentModelCount, 4)
        XCTAssertEqual(store.persistentModelCount, 4)
    }

    @MainActor
    func testPersistentStoreDirectoryIsProtectedAndExcludedFromBackup() throws {
        let storeURL = try temporaryStoreURL()
        let store = try MessagingStore(storeURL: storeURL)
        let directory = store.storeURL.deletingLastPathComponent()
        let values = try directory.resourceValues(forKeys: [.isExcludedFromBackupKey])
        let defaultURL = try MessagingStore.defaultStoreURL()

        XCTAssertEqual(store.fileProtectionType, .completeUntilFirstUserAuthentication)
        XCTAssertEqual(values.isExcludedFromBackup, true)
        XCTAssertTrue(store.storeURL.path.contains("AgentMessaging"))
        XCTAssertTrue(defaultURL.path.contains("Application Support/Brunn/AgentMessaging"))
    }

    @MainActor
    func testDifferentSessionNeverPaintsPriorAccountCache() throws {
        let storeURL = try temporaryStoreURL()
        do {
            let store = try MessagingStore(storeURL: storeURL)
            try store.bindValidatedAccount(
                accountID: accountA,
                sessionFingerprint: fingerprintA
            )
            try store.setMessagingEnabled(true)
            try store.applyInboxDelta(sync(cursor: 4, conversations: [conversation()]))
            XCTAssertEqual(try store.conversations().count, 1)
        }

        let reopened = try MessagingStore(storeURL: storeURL)
        XCTAssertFalse(try reopened.activateCachedSession(sessionFingerprint: fingerprintB))
        XCTAssertFalse(try reopened.isMessagingEnabled())
        XCTAssertTrue(try reopened.conversations().isEmpty)
        XCTAssertTrue(try reopened.messages(conversationID: conversationID).isEmpty)

        XCTAssertTrue(try reopened.activateCachedSession(sessionFingerprint: fingerprintA))
        XCTAssertTrue(try reopened.isMessagingEnabled())
        XCTAssertEqual(try reopened.conversations().map(\.conversationID), [conversationID])
    }

#if DEBUG
    @MainActor
    func testDebugStoreNamespacesIsolateAndSameNamespaceSurvivesRelaunch() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("AgentMessagingNamespaceTests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let namespaceA = "gate12c-a-\(UUID().uuidString)"
        let namespaceB = "gate12c-b-\(UUID().uuidString)"
        let urlA = try MessagingStore.storeURL(
            applicationSupportRoot: root,
            environment: [MessagingStore.namespaceEnvironmentKey: namespaceA]
        )
        let urlB = try MessagingStore.storeURL(
            applicationSupportRoot: root,
            environment: [MessagingStore.namespaceEnvironmentKey: namespaceB]
        )

        XCTAssertNotEqual(urlA, urlB)
        XCTAssertTrue(urlA.path.contains(namespaceA))
        XCTAssertTrue(urlB.path.contains(namespaceB))
        XCTAssertNil(MessagingStore.validatedStoreNamespace("../escape"))
        XCTAssertNil(MessagingStore.validatedStoreNamespace("nested/path"))
        XCTAssertNil(MessagingStore.validatedStoreNamespace(String(repeating: "x", count: 65)))
        XCTAssertNil(MessagingStore.validatedStoreNamespace("méssaging"))

        do {
            let storeA = try MessagingStore(storeURL: urlA)
            try storeA.bindValidatedAccount(
                accountID: accountA,
                sessionFingerprint: fingerprintA
            )
            try storeA.setMessagingEnabled(true)
            try storeA.applyInboxDelta(sync(cursor: 4, conversations: [conversation()]))
        }
        do {
            let storeB = try MessagingStore(storeURL: urlB)
            try storeB.bindValidatedAccount(
                accountID: accountA,
                sessionFingerprint: fingerprintA
            )
            XCTAssertFalse(try storeB.isMessagingEnabled())
            XCTAssertTrue(try storeB.conversations().isEmpty)
        }

        let reopenedA = try MessagingStore(storeURL: urlA)
        XCTAssertTrue(try reopenedA.activateCachedSession(sessionFingerprint: fingerprintA))
        XCTAssertTrue(try reopenedA.isMessagingEnabled())
        XCTAssertEqual(try reopenedA.conversations().map(\.conversationID), [conversationID])
    }
#endif

    @MainActor
    func testOutboxBytesAndClientKeySurviveReopenAndSendingBecomesQueued() throws {
        let storeURL = try temporaryStoreURL()
        let request = sendRequest()
        let bytes = try request.encodedData()
        do {
            let store = try MessagingStore(storeURL: storeURL)
            try store.bindValidatedAccount(
                accountID: accountA,
                sessionFingerprint: fingerprintA
            )
            let record = try store.enqueueOptimisticMessage(
                conversationID: conversationID,
                senderAgentID: "owner",
                request: request,
                exactRequestData: bytes,
                createdAt: "2026-08-27T15:00:00Z"
            )
            XCTAssertEqual(record.deliveryState, .sending)
        }

        let reopened = try MessagingStore(storeURL: storeURL)
        XCTAssertTrue(try reopened.activateCachedSession(sessionFingerprint: fingerprintA))
        let record = try XCTUnwrap(reopened.pendingOutbox().only)

        XCTAssertEqual(record.clientKey, clientKey)
        XCTAssertEqual(record.exactRequestData, bytes)
        XCTAssertEqual(record.deliveryState, .queued)
    }

    @MainActor
    func testInboxSyncReconcilesAmbiguousSendExactlyOnce() throws {
        let store = try boundMemoryStore()
        let request = sendRequest()
        let bytes = try request.encodedData()
        _ = try store.enqueueOptimisticMessage(
            conversationID: conversationID,
            senderAgentID: "owner",
            request: request,
            exactRequestData: bytes,
            createdAt: "2026-08-27T15:00:00Z"
        )
        let canonical = message(
            messageID: "018f89c9-4444-7d57-9b8d-340e16d879f2",
            clientKey: clientKey,
            fromAgentID: "owner",
            seq: 1,
            cursor: 5
        )

        try store.applyInboxDelta(sync(cursor: 5, messages: [canonical]))
        try store.applyInboxDelta(sync(cursor: 5, messages: [canonical]))
        let records = try store.messages(conversationID: conversationID)

        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records[0].messageID, canonical.messageID)
        XCTAssertEqual(records[0].clientKey, clientKey)
        XCTAssertEqual(records[0].deliveryState, .canonical)
        XCTAssertNil(records[0].exactRequestData)
    }

    @MainActor
    func testContinuationSendAcknowledgementRemainsInRequestedLogicalThread() throws {
        let store = try boundMemoryStore()
        let parentID = "018f89d0-1111-7abc-8def-000000000001"
        let childID = "018f89d1-2222-7abc-8def-000000000002"
        try store.applyInboxDelta(sync(
            cursor: 8,
            conversations: [conversation(
                conversationID: parentID,
                status: "closed",
                lastSeq: 500
            )]
        ))
        let childMessage = message(
            conversationID: childID,
            messageID: "child-ack",
            clientKey: clientKey,
            fromAgentID: "owner",
            seq: 1,
            cursor: 9
        )

        try store.applySendAcknowledgement(
            MessagingSendResponse(
                conversationID: childID,
                sequence: 1,
                message: childMessage,
                duplicate: false
            ),
            requestedConversationID: parentID
        )
        let logical = try store.logicalThreadMessages(containing: parentID)

        XCTAssertEqual(logical.count, 1)
        XCTAssertEqual(logical[0].wireIdentity, "\(childID):1")
        XCTAssertEqual(try store.inboxCursor(), 8)
    }

    @MainActor
    func testThreadSyncNeverAdvancesInboxCursor() throws {
        let store = try boundMemoryStore()
        try store.applyInboxDelta(sync(cursor: 10))
        try store.applyInboxDelta(sync(cursor: 8))
        XCTAssertEqual(try store.inboxCursor(), 10)

        try store.applyThreadDelta(sync(
            cursor: 25,
            messages: [message(
                messageID: "018f89ca-5555-76a8-b77c-f2b6acfcab17",
                clientKey: nil,
                fromAgentID: nil,
                seq: 1,
                cursor: 25
            )]
        ))

        XCTAssertEqual(try store.inboxCursor(), 10)
        XCTAssertEqual(try store.messages(conversationID: conversationID).count, 1)
    }

    @MainActor
    func testMutationAcknowledgementsNeverAdvanceInboxCursor() throws {
        let store = try boundMemoryStore()
        try store.applyInboxDelta(sync(cursor: 10))
        let created = conversation(lastSeq: 0, latestSyncCursor: 25)

        try store.applyCreateAcknowledgement(
            MessagingCreateConversationResponse(
                conversationID: conversationID,
                conversation: created,
                duplicate: false
            ),
            asOf: "2026-08-27T15:00:00Z"
        )
        XCTAssertEqual(try store.inboxCursor(), 10)

        try store.applyReadAcknowledgement(MessagingReadResponse(
            conversationID: conversationID,
            lastReadSeq: 0,
            cursor: 30,
            duplicate: false
        ))
        XCTAssertEqual(try store.inboxCursor(), 10)

        let intervening = message(
            messageID: "unseen-intervening-event",
            clientKey: nil,
            fromAgentID: "echo",
            seq: 1,
            cursor: 11
        )
        try store.applyInboxDelta(sync(cursor: 11, messages: [intervening]))
        XCTAssertEqual(try store.inboxCursor(), 11)
        XCTAssertEqual(
            try store.messages(conversationID: conversationID).map(\.messageID),
            ["unseen-intervening-event"]
        )
    }

    @MainActor
    func testThreadRefreshUsesSequenceCursorWithoutSendingInboxCursor() async throws {
        let store = try boundMemoryStore()
        try store.applyInboxDelta(sync(cursor: 10))
        let observation = MessagingSyncObservation()
        let response = sync(cursor: 25)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(sync: { request in
                await observation.record(request)
                return response
            }, send: { _, _ in
                throw MessagingTransportError.operationNotConfigured
            })
        )

        _ = try await controller.refreshThread(conversationID: conversationID)
        let observedRequest = await observation.value
        let request = try XCTUnwrap(observedRequest)

        XCTAssertEqual(request.cursor, 0)
        XCTAssertEqual(request.conversationID, conversationID)
        XCTAssertEqual(request.afterSequence, 0)
        XCTAssertEqual(try store.inboxCursor(), 10)
    }

    @MainActor
    func testThreadRefreshDrainsEveryHasMorePageBySequence() async throws {
        let store = try boundMemoryStore()
        // A cold deep link can materialize the focused boundary row before the
        // full thread. The drain must restart at zero because 1 ... 499 are not
        // yet a contiguous local prefix.
        try store.applyInboxDelta(sync(
            cursor: 900,
            messages: [message(
                messageID: "page-three-500",
                clientKey: nil,
                fromAgentID: "echo",
                seq: 500,
                cursor: 500
            )],
            conversations: [conversation(lastSeq: 500)]
        ))
        let pages = [
            ThreadPageKey(conversationID: conversationID, afterSequence: 0): sync(
                cursor: 900,
                messages: (1 ... 200).map { index in
                    message(
                        messageID: "page-one-\(index)",
                        clientKey: nil,
                        fromAgentID: "echo",
                        seq: Int64(index),
                        cursor: Int64(index)
                    )
                },
                conversations: [conversation(lastSeq: 500)],
                hasMore: true
            ),
            ThreadPageKey(conversationID: conversationID, afterSequence: 200): sync(
                cursor: 900,
                messages: (201 ... 400).map { index in
                    message(
                        messageID: "page-two-\(index)",
                        clientKey: nil,
                        fromAgentID: "echo",
                        seq: Int64(index),
                        cursor: Int64(index)
                    )
                },
                hasMore: true
            ),
            ThreadPageKey(conversationID: conversationID, afterSequence: 400): sync(
                cursor: 900,
                messages: (401 ... 500).map { index in
                    message(
                        messageID: "page-three-\(index)",
                        clientKey: nil,
                        fromAgentID: "echo",
                        seq: Int64(index),
                        cursor: Int64(index)
                    )
                }
            ),
        ]
        let pager = ThreadPager(responses: pages)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { request in try await pager.response(for: request) },
                send: { _, _ in throw MessagingTransportError.operationNotConfigured }
            )
        )

        _ = try await controller.refreshThread(conversationID: conversationID)
        let requests = await pager.recordedRequests

        XCTAssertEqual(requests.map(\.afterSequence), [0, 200, 400])
        XCTAssertTrue(requests.allSatisfy { $0.cursor == 0 })
        XCTAssertEqual(controller.selectedMessages.count, 500)
        XCTAssertEqual(controller.selectedMessages.last?.sequence, 500)
        XCTAssertEqual(try store.inboxCursor(), 900)
    }

    @MainActor
    func testSparseContinuationRefreshBackfillsEveryPhysicalMember() async throws {
        let store = try boundMemoryStore()
        let parentID = "018f89d0-1111-7abc-8def-000000000001"
        let childID = "018f89d1-2222-7abc-8def-000000000002"
        let parent = conversation(
            conversationID: parentID,
            status: "closed",
            lastSeq: 500,
            continuationID: childID,
            latestSyncCursor: 500
        )
        let child = conversation(
            conversationID: childID,
            lastSeq: 500,
            continuesFrom: parentID,
            latestSyncCursor: 1_000
        )

        func pageMessages(
            conversationID: String,
            prefix: String,
            range: ClosedRange<Int>,
            cursorOffset: Int
        ) -> [MessagingMessage] {
            range.map { index in
                message(
                    conversationID: conversationID,
                    messageID: "\(prefix)-\(index)",
                    clientKey: nil,
                    fromAgentID: "echo",
                    seq: Int64(index),
                    cursor: Int64(cursorOffset + index)
                )
            }
        }

        // Reproduce a cold deep link whose inbox delta materialized only each
        // physical member's boundary row before historical thread pagination.
        try store.applyInboxDelta(sync(
            cursor: 1_000,
            messages: [
                message(
                    conversationID: parentID,
                    messageID: "parent-500",
                    clientKey: nil,
                    fromAgentID: "echo",
                    seq: 500,
                    cursor: 500
                ),
                message(
                    conversationID: childID,
                    messageID: "child-500",
                    clientKey: nil,
                    fromAgentID: "echo",
                    seq: 500,
                    cursor: 1_000
                ),
            ],
            conversations: [parent, child]
        ))
        let initialInboxCursor = try store.inboxCursor()
        let pages = [
            ThreadPageKey(conversationID: parentID, afterSequence: 0): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: parentID,
                    prefix: "parent",
                    range: 1 ... 200,
                    cursorOffset: 0
                ),
                conversations: [parent],
                hasMore: true
            ),
            ThreadPageKey(conversationID: parentID, afterSequence: 200): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: parentID,
                    prefix: "parent",
                    range: 201 ... 400,
                    cursorOffset: 0
                ),
                hasMore: true
            ),
            ThreadPageKey(conversationID: parentID, afterSequence: 400): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: parentID,
                    prefix: "parent",
                    range: 401 ... 500,
                    cursorOffset: 0
                )
            ),
            ThreadPageKey(conversationID: childID, afterSequence: 0): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: childID,
                    prefix: "child",
                    range: 1 ... 200,
                    cursorOffset: 500
                ),
                conversations: [child],
                hasMore: true
            ),
            ThreadPageKey(conversationID: childID, afterSequence: 200): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: childID,
                    prefix: "child",
                    range: 201 ... 400,
                    cursorOffset: 500
                ),
                hasMore: true
            ),
            ThreadPageKey(conversationID: childID, afterSequence: 400): sync(
                cursor: 1_000,
                messages: pageMessages(
                    conversationID: childID,
                    prefix: "child",
                    range: 401 ... 500,
                    cursorOffset: 500
                )
            ),
        ]
        let pager = ThreadPager(responses: pages)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { request in try await pager.response(for: request) },
                send: { _, _ in throw MessagingTransportError.operationNotConfigured }
            )
        )

        _ = try await controller.refreshThread(conversationID: parentID)

        let requests = await pager.recordedRequests
        XCTAssertEqual(
            requests.filter { $0.conversationID == parentID }.map(\.afterSequence),
            [0, 200, 400]
        )
        XCTAssertEqual(
            requests.filter { $0.conversationID == childID }.map(\.afterSequence),
            [0, 200, 400]
        )
        XCTAssertTrue(requests.allSatisfy { $0.cursor == 0 && $0.limit == 200 })

        let wireIdentities = controller.selectedMessages.map(\.wireIdentity)
        let expectedWireIdentities = (1 ... 500).map { "\(parentID):\($0)" }
            + (1 ... 500).map { "\(childID):\($0)" }
        XCTAssertEqual(wireIdentities, expectedWireIdentities)
        XCTAssertEqual(Set(wireIdentities).count, 1_000)
        XCTAssertEqual(try store.inboxCursor(), initialInboxCursor)
    }

    @MainActor
    func testInboxRefreshDrainsHasMoreWithoutSkippingCursor() async throws {
        let store = try boundMemoryStore()
        let first = sync(
            cursor: 100,
            messages: [message(
                messageID: "inbox-first",
                clientKey: nil,
                fromAgentID: "echo",
                seq: 1,
                cursor: 100
            )],
            hasMore: true
        )
        let second = sync(
            cursor: 200,
            messages: [message(
                messageID: "inbox-second",
                clientKey: nil,
                fromAgentID: "echo",
                seq: 2,
                cursor: 200
            )]
        )
        let pager = InboxPager(responses: [0: first, 100: second])
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { request in try await pager.response(for: request) },
                send: { _, _ in throw MessagingTransportError.operationNotConfigured }
            )
        )

        let aggregate = try await controller.refreshInbox()
        let requests = await pager.recordedRequests

        XCTAssertEqual(requests.map(\.cursor), [0, 100])
        XCTAssertTrue(requests.allSatisfy { $0.conversationID == nil })
        XCTAssertEqual(aggregate.messages.map(\.messageID), ["inbox-first", "inbox-second"])
        XCTAssertFalse(aggregate.hasMore)
        XCTAssertEqual(try store.inboxCursor(), 200)
        XCTAssertEqual(try store.messages(conversationID: conversationID).count, 2)
    }

    @MainActor
    func testLinkedThreadRefreshKeepsEarlierActivityWhenDiscoveredPredecessorIsEmpty() async throws {
        let store = try boundMemoryStore()
        let parentID = "018f89d0-1111-7abc-8def-000000000001"
        let childID = "018f89d1-2222-7abc-8def-000000000002"
        let childMessage = message(
            conversationID: childID,
            messageID: "child-before-empty-parent",
            clientKey: nil,
            fromAgentID: "echo",
            seq: 1,
            cursor: 51
        )
        let pages = [
            ThreadPageKey(conversationID: childID, afterSequence: 0): sync(
                cursor: 51,
                messages: [childMessage],
                conversations: [conversation(
                    conversationID: childID,
                    continuesFrom: parentID,
                    latestSyncCursor: 51
                )]
            ),
            ThreadPageKey(conversationID: parentID, afterSequence: 0): sync(
                cursor: 51,
                conversations: [conversation(
                    conversationID: parentID,
                    status: "closed",
                    lastSeq: 500,
                    continuationID: childID,
                    latestSyncCursor: 50
                )]
            ),
        ]
        let pager = ThreadPager(responses: pages)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { request in try await pager.response(for: request) },
                send: { _, _ in throw MessagingTransportError.operationNotConfigured }
            )
        )

        let aggregate = try await controller.refreshThread(conversationID: childID)
        let requests = await pager.recordedRequests

        XCTAssertEqual(requests.compactMap(\.conversationID), [childID, parentID])
        XCTAssertEqual(aggregate.messages.map(\.messageID), [childMessage.messageID])
        XCTAssertEqual(Set(aggregate.conversations.map(\.conversationID)), [parentID, childID])
        XCTAssertEqual(controller.selectedMessages.map(\.wireIdentity), ["\(childID):1"])
    }

    @MainActor
    func testLogicalContinuationThreadAggregatesThousandRowsFromEitherMember() async throws {
        let store = try boundMemoryStore()
        let parentID = "018f89d0-1111-7abc-8def-000000000001"
        let childID = "018f89d1-2222-7abc-8def-000000000002"
        let parent = conversation(
            conversationID: parentID,
            status: "closed",
            lastSeq: 500,
            continuationID: childID
        )
        let child = conversation(
            conversationID: childID,
            lastSeq: 500,
            continuesFrom: parentID
        )
        let parentMessages = (1 ... 500).map { index in
            message(
                conversationID: parentID,
                messageID: "parent-\(index)",
                clientKey: nil,
                fromAgentID: "echo",
                seq: Int64(index),
                cursor: Int64(index)
            )
        }
        let childMessages = (1 ... 500).map { index in
            message(
                conversationID: childID,
                messageID: "child-\(index)",
                clientKey: nil,
                fromAgentID: "echo",
                seq: Int64(index),
                cursor: Int64(500 + index)
            )
        }
        try store.applyInboxDelta(sync(
            cursor: 1_000,
            messages: parentMessages + childMessages,
            conversations: [parent, child]
        ))
        let readObservation = MessagingReadObservation()
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { _ in throw MessagingTransportError.operationNotConfigured },
                send: { _, _ in throw MessagingTransportError.operationNotConfigured },
                markRead: { conversationID, request in
                    await readObservation.record(
                        conversationID: conversationID,
                        sequence: request.lastReadSeq
                    )
                    return MessagingReadResponse(
                        conversationID: conversationID,
                        lastReadSeq: request.lastReadSeq,
                        cursor: 1_001,
                        duplicate: false
                    )
                }
            )
        )

        try controller.selectConversation(childID)
        let fromChild = controller.selectedMessages
        try controller.selectConversation(parentID)
        let fromParent = controller.selectedMessages

        XCTAssertEqual(fromChild.count, 1_000)
        XCTAssertEqual(fromParent.map(\.wireIdentity), fromChild.map(\.wireIdentity))
        XCTAssertEqual(fromChild.first?.conversationID, parentID)
        XCTAssertEqual(fromChild.first?.sequence, 1)
        XCTAssertEqual(fromChild.last?.conversationID, childID)
        XCTAssertEqual(fromChild.last?.sequence, 500)
        XCTAssertEqual(Set(fromChild.map(\.wireIdentity)).count, 1_000)
        XCTAssertEqual(try controller.conversationChain(containing: childID), [parentID, childID])
        XCTAssertEqual(try controller.logicalRootConversationID(for: childID), parentID)
        XCTAssertEqual(try controller.activeConversationID(containing: parentID), childID)
        XCTAssertEqual(controller.selectedActiveConversationID, childID)

        let lastVisible = try XCTUnwrap(fromChild.last)
        try await controller.markRead(through: lastVisible)
        let readTarget = await readObservation.value
        XCTAssertEqual(readTarget?.conversationID, childID)
        XCTAssertEqual(readTarget?.sequence, 500)
        XCTAssertEqual(try store.inboxCursor(), 1_000)
    }

    @MainActor
    func testThousandMessagesApplyWithOneSave() throws {
        let store = try boundMemoryStore()
        let messages = (1 ... 1_000).map { index in
            message(
                messageID: String(format: "018f89cb-%04x-7abc-8def-%012x", index, index),
                clientKey: nil,
                fromAgentID: index.isMultiple(of: 2) ? "owner" : "echo",
                seq: Int64(index),
                cursor: Int64(index)
            )
        }
        let saveCount = store.saveCount

        try store.applyInboxDelta(sync(cursor: 1_000, messages: messages))

        XCTAssertEqual(store.saveCount - saveCount, 1)
        XCTAssertEqual(try store.messages(conversationID: conversationID).count, 1_000)
        XCTAssertEqual(try store.inboxCursor(), 1_000)
    }

    @MainActor
    func testProtectedThousandRowCacheActivatesUnderThreeHundredMilliseconds() throws {
        let storeURL = try temporaryStoreURL()
        do {
            let store = try MessagingStore(storeURL: storeURL)
            try store.bindValidatedAccount(
                accountID: accountA,
                sessionFingerprint: fingerprintA
            )
            try store.setMessagingEnabled(true)
            let messages = (1 ... 1_000).map { index in
                message(
                    messageID: String(
                        format: "018f89cd-%04x-7abc-8def-%012x",
                        index,
                        index
                    ),
                    clientKey: nil,
                    fromAgentID: index.isMultiple(of: 2) ? "owner" : "echo",
                    seq: Int64(index),
                    cursor: Int64(index)
                )
            }
            try store.applyInboxDelta(sync(cursor: 1_000, messages: messages))
        }

        let reopened = try MessagingStore(storeURL: storeURL)
        XCTAssertEqual(
            reopened.fileProtectionType,
            .completeUntilFirstUserAuthentication
        )

        // Activation scans the cached messages to recover interrupted outbox
        // sends, so this timed path exercises all 1,000 persisted rows.
        let startedAt = ProcessInfo.processInfo.systemUptime
        let activated = try reopened.activateCachedSession(
            sessionFingerprint: fingerprintA
        )
        let activationMilliseconds = (
            ProcessInfo.processInfo.systemUptime - startedAt
        ) * 1_000

        XCTAssertTrue(activated)
        XCTAssertLessThan(
            activationMilliseconds,
            300,
            "Protected 1,000-row cache activation took \(activationMilliseconds) ms."
        )
        XCTAssertTrue(try reopened.isMessagingEnabled())
        XCTAssertEqual(
            try reopened.messages(conversationID: conversationID).count,
            1_000
        )
    }

    @MainActor
    func testForegroundFlushDoesNotResendAnInFlightMessage() async throws {
        let store = try boundMemoryStore()
        let request = sendRequest()
        let bytes = try request.encodedData()
        let record = try store.enqueueOptimisticMessage(
            conversationID: conversationID,
            senderAgentID: "owner",
            request: request,
            exactRequestData: bytes,
            createdAt: "2026-08-27T15:00:00Z"
        )
        XCTAssertEqual(record.deliveryState, .sending)

        let counter = InvocationCounter()
        let acknowledgement = sendResponse(message: message(
            messageID: "018f89cc-6666-7d03-827f-3c35cfca65d8",
            clientKey: clientKey,
            fromAgentID: "owner",
            seq: 1,
            cursor: 1
        ))
        let emptySync = sync(cursor: 0)
        let controller = MessagingController(
            store: store,
            transport: MessagingTransportOperations(
                sync: { _ in emptySync },
                send: { _, _ in
                    await counter.increment()
                    return acknowledgement
                }
            )
        )

        try await controller.flushOutbox()

        let invocationCount = await counter.count
        XCTAssertEqual(invocationCount, 0)
        XCTAssertEqual(try store.pendingOutbox().only?.deliveryState, .sending)
    }

    @MainActor
    func testControllerPersistsExactRequestBeforeTransport() async throws {
        let store = try boundMemoryStore()
        let observation = SendObservation()
        let expectedClientKey = clientKey
        let emptySync = sync(cursor: 0)
        let acknowledgement = sendResponse(
            message: message(
                messageID: "018f89cc-6666-7d03-827f-3c35cfca65d7",
                clientKey: clientKey,
                fromAgentID: "owner",
                seq: 1,
                cursor: 1
            )
        )
        let transport = MessagingTransportOperations(
            sync: { _ in emptySync },
            send: { conversationID, data in
                let persisted = try await MainActor.run { () throws -> PersistedSend in
                    guard let record = try store.pendingOutbox().only else {
                        throw MessagingStoreError.corruptStoredValue
                    }
                    return PersistedSend(
                        data: record.exactRequestData,
                        clientKey: record.clientKey,
                        state: record.deliveryState
                    )
                }
                await observation.record(
                    conversationID: conversationID,
                    transportedData: data,
                    persistedData: persisted.data,
                    clientKey: persisted.clientKey,
                    state: persisted.state
                )
                return acknowledgement
            }
        )
        let controller = MessagingController(
            store: store,
            transport: transport,
            clientKeyGenerator: { expectedClientKey }
        )

        _ = try await controller.send(
            conversationID: conversationID,
            senderAgentID: "owner",
            kind: .question,
            bodyMarkdown: "Can you echo this?",
            expectsReply: true
        )
        let observedValue = await observation.value
        let recorded = try XCTUnwrap(observedValue)
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: recorded.transportedData) as? [String: Any]
        )

        XCTAssertEqual(recorded.transportedData, recorded.persistedData)
        XCTAssertEqual(recorded.clientKey, clientKey)
        XCTAssertEqual(recorded.state, .sending)
        XCTAssertNil(object["from"])
        XCTAssertNil(object["from_agent_id"])
        XCTAssertEqual(try store.messages(conversationID: conversationID).count, 1)
    }

    @MainActor
    private func boundMemoryStore() throws -> MessagingStore {
        let store = try MessagingStore(isStoredInMemoryOnly: true)
        try store.bindValidatedAccount(
            accountID: accountA,
            sessionFingerprint: fingerprintA
        )
        return store
    }

    private func temporaryStoreURL() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("AgentMessagingTests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root.appendingPathComponent("messaging.sqlite", isDirectory: false)
    }

    private func sendRequest() -> MessagingSendRequest {
        MessagingSendRequest(
            clientKey: clientKey,
            kind: .question,
            bodyMarkdown: "Can you echo this?",
            refs: [],
            expectsReply: true
        )
    }

    private func conversation(
        conversationID: String? = nil,
        status: String = "open",
        lastSeq: Int64 = 1,
        continuesFrom: String? = nil,
        continuationID: String? = nil,
        latestSyncCursor: Int64 = 5
    ) -> MessagingConversation {
        MessagingConversation(
            conversationID: conversationID ?? self.conversationID,
            conversationKind: "direct",
            subject: nil,
            status: status,
            participants: [
                MessagingParticipant(agentID: "owner", role: "participant"),
                MessagingParticipant(agentID: "echo", role: "participant"),
            ],
            lastSeq: lastSeq,
            lastMessageAt: "2026-08-27T15:00:00Z",
            lastReadSeq: 0,
            unreadCount: 1,
            needsHuman: false,
            continuesFrom: continuesFrom,
            continuationID: continuationID,
            latestSyncCursor: latestSyncCursor
        )
    }

    private func message(
        conversationID: String? = nil,
        messageID: String,
        clientKey: String?,
        fromAgentID: String?,
        seq: Int64,
        cursor: Int64
    ) -> MessagingMessage {
        MessagingMessage(
            conversationID: conversationID ?? self.conversationID,
            sequence: seq,
            messageID: messageID,
            fromAgentID: fromAgentID,
            clientKey: clientKey,
            kind: "text",
            bodyMarkdown: "message \(seq)",
            refs: [],
            expectsReply: false,
            syncCursor: cursor,
            createdAt: "2026-08-27T15:00:00Z"
        )
    }

    private func sync(
        cursor: Int64,
        messages: [MessagingMessage] = [],
        conversations: [MessagingConversation] = [],
        hasMore: Bool = false
    ) -> MessagingSyncResponse {
        MessagingSyncResponse(
            status: "complete",
            cursor: cursor,
            hasMore: hasMore,
            messages: messages,
            conversations: conversations,
            presence: [],
            unread: [:],
            asOf: "2026-08-27T15:00:00Z"
        )
    }

    private func sendResponse(message: MessagingMessage) -> MessagingSendResponse {
        MessagingSendResponse(
            conversationID: conversationID,
            sequence: message.sequence,
            message: message,
            duplicate: false
        )
    }
}

private actor SendObservation {
    struct Value: Sendable {
        let conversationID: String
        let transportedData: Data
        let persistedData: Data?
        let clientKey: String?
        let state: MessagingDeliveryState
    }

    private(set) var value: Value?

    func record(
        conversationID: String,
        transportedData: Data,
        persistedData: Data?,
        clientKey: String?,
        state: MessagingDeliveryState
    ) {
        value = Value(
            conversationID: conversationID,
            transportedData: transportedData,
            persistedData: persistedData,
            clientKey: clientKey,
            state: state
        )
    }
}

private actor InvocationCounter {
    private(set) var count = 0

    func increment() {
        count += 1
    }
}

private actor MessagingSyncObservation {
    private(set) var value: MessagingSyncRequest?

    func record(_ request: MessagingSyncRequest) {
        value = request
    }
}

private struct ThreadPageKey: Hashable, Sendable {
    let conversationID: String
    let afterSequence: Int64
}

private actor ThreadPager {
    private let responses: [ThreadPageKey: MessagingSyncResponse]
    private(set) var recordedRequests: [MessagingSyncRequest] = []

    init(responses: [ThreadPageKey: MessagingSyncResponse]) {
        self.responses = responses
    }

    func response(for request: MessagingSyncRequest) throws -> MessagingSyncResponse {
        recordedRequests.append(request)
        let key = ThreadPageKey(
            conversationID: request.conversationID ?? "",
            afterSequence: request.afterSequence ?? 0
        )
        guard let response = responses[key] else {
            throw MessagingTransportError.operationNotConfigured
        }
        return response
    }
}

private actor InboxPager {
    private let responses: [Int64: MessagingSyncResponse]
    private(set) var recordedRequests: [MessagingSyncRequest] = []

    init(responses: [Int64: MessagingSyncResponse]) {
        self.responses = responses
    }

    func response(for request: MessagingSyncRequest) throws -> MessagingSyncResponse {
        recordedRequests.append(request)
        guard let response = responses[request.cursor] else {
            throw MessagingTransportError.operationNotConfigured
        }
        return response
    }
}

private actor MessagingReadObservation {
    struct Value: Sendable {
        let conversationID: String
        let sequence: Int64
    }

    private(set) var value: Value?

    func record(conversationID: String, sequence: Int64) {
        value = Value(conversationID: conversationID, sequence: sequence)
    }
}

private struct PersistedSend: Sendable {
    let data: Data?
    let clientKey: String?
    let state: MessagingDeliveryState
}

private extension Array {
    var only: Element? { count == 1 ? self[0] : nil }
}
