import Foundation
import XCTest
@testable import Brunn

@MainActor
final class DreamerReviewTests: XCTestCase {
    func testDecisionConfirmationDecodesInitialAndIdempotentReplayResponses() throws {
        // The live decision endpoint returns the action string; the complete
        // decision object belongs to Review history, not this confirmation.
        let responses = [
            #"{"status":"complete","data":{"saved":true,"decision":"approve","application_status":"approved_held","message":"Approved. Application is held until publication is explicitly enabled.","state_version":64}}"#,
            #"{"status":"complete","data":{"saved":true,"decision":"approve","application_status":"approved_held","message":"Decision already recorded.","state_version":64}}"#,
        ]
        for response in responses {
            let result = try JSONDecoder().decode(WorkspaceEnvelope<DreamerDecisionResult>.self, from: Data(response.utf8)).data
            XCTAssertEqual(result.saved, true)
            XCTAssertEqual(result.decision, "approve")
            XCTAssertEqual(result.applicationStatus, "approved_held")
            XCTAssertEqual(result.stateVersion, 64)
            XCTAssertFalse(try XCTUnwrap(result.message).isEmpty)
        }
    }

    func testManagedSummaryTablePreservesSevenStopsAndExactCandidateBytes() throws {
        let markdown = DreamerReviewStore.uiTestLocationSummary
        let candidate = DreamerReviewCandidate(bodyMD: "Earlier draft", beforeMD: "Before", afterMD: markdown,
                                              targetPath: "derived/location/days/2040-02-03.md")
        XCTAssertTrue(candidate.isManagedSummary)
        XCTAssertEqual(candidate.summaryMD, markdown)
        let blocks = ReviewSummaryBlock.parse(markdown)
        XCTAssertEqual(blocks.first, .heading("February 3 · UTC"))
        guard case .stops(let stops) = blocks[1] else { return XCTFail("Expected readable stop rows") }
        XCTAssertEqual(stops.count, 7)
        XCTAssertEqual(stops[2], .init(when: "About 10:41–10:44", whereMD: "Cafe, brief stop"))
        XCTAssertEqual(stops.last, .init(when: "About 21:27–21:28", whereMD: "Station entrance"))
        XCTAssertEqual(candidate.afterMD, markdown)
        XCTAssertEqual(candidate.bodyMD, "Earlier draft")
        for path in ["sources/Places.md", "derived/location-unrelated/file.md"] {
            XCTAssertFalse(DreamerReviewCandidate(bodyMD: markdown, beforeMD: nil, afterMD: nil, targetPath: path).isManagedSummary)
        }
        let unrelatedTable = "| Key | Value |\n| --- | --- |\n| Original | Intact |"
        let malformedTable = "| When | Where |\n| --- | --- |\n| 12:00 | A | Additional data |"
        XCTAssertEqual(ReviewSummaryBlock.parse(unrelatedTable), [.paragraph(unrelatedTable)])
        XCTAssertEqual(ReviewSummaryBlock.parse(malformedTable), [.paragraph(malformedTable)])
    }

    func testNativeRoutesAndPinnedSource() throws {
        XCTAssertEqual(AppRoute(url: URL(string: "brunn://review")!), .review)
        XCTAssertEqual(AppRoute(url: URL(string: "brunn://dreams")!), .review)
        for link in ["https://brunn.ai/dreams", "brunn://review/other", "brunn://review?token=secret", "brunn://owner@review", "brunn://review#other"] {
            XCTAssertNil(AppRoute(url: URL(string: link)!))
        }
        let source = WorkspaceEntryRequest(reference: "entry:evidence", version: 12, title: "Evidence")
        XCTAssertEqual(source.version, 12)
        XCTAssertTrue(source.pathCandidates.isEmpty)
    }

    func testCompleteProposalAndApprovalSafety() throws {
        let item = try reviewFixture().items[0]
        XCTAssertEqual(item.bodyMD, String(repeating: "Complete original question. ", count: 200))
        XCTAssertEqual(item.sources[0].version, 12)
        XCTAssertEqual(item.candidate?.afterMD, "Exact proposed content.")
        XCTAssertNil(item.approvalBlock)
        for (field, value) in [("stale", true as Any), ("reviewable", false), ("candidate", NSNull()), ("sources", [] as [String]), ("status", "approved_held"), ("status", "needs_changes")] {
            XCTAssertNotNil(try reviewFixture(itemOverrides: [field: value]).items[0].approvalBlock)
        }
    }

    func testRefreshRetainsSnapshotAndDraftUntilExplicitReview() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        let old = try XCTUnwrap(store.data?.items.first)
        store.select(old)
        store.drafts[old.id] = .init(comment: "Keep this context", correction: "")
        await harness.replace(try reviewFixture(version: 4, itemOverrides: ["body_md": "Updated proposal.", "candidate_hash": "sha256:changed"]))
        await store.refresh(api: api, userID: "owner")
        XCTAssertEqual(store.selected?.bodyMD, old.bodyMD)
        XCTAssertEqual(store.displayedItem?.bodyMD, "Updated proposal.")
        XCTAssertTrue(store.hasReplacement)
        XCTAssertTrue(store.changed)
        XCTAssertTrue(store.decisionsDisabled)
        store.selectUpdated()
        XCTAssertEqual(store.selected?.bodyMD, "Updated proposal.")
        XCTAssertEqual(store.drafts[old.id]?.comment, "Keep this context")
        XCTAssertFalse(store.changed)
        XCTAssertFalse(store.hasReplacement)
    }

    func testRefreshSignalDuringRequestFetchesReplacementAndKeepsDecisionIdentityPinned() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        store.drafts["item-1"] = .init(comment: "Keep my draft", correction: "")
        let gate = ReviewAsyncGate()
        await harness.holdNextRead(gate)
        let pending = Task { await store.refresh(api: api, userID: "owner") }
        await gate.waitUntilSuspended()
        await harness.replace(try reviewFixture(version: 4, itemOverrides: ["body_md": "Fresh replacement.", "run_version": 7, "candidate_hash": "sha256:replacement"]))
        await store.refresh(api: api, userID: "owner")
        await gate.release()
        await pending.value
        let readCount = await harness.readCount
        XCTAssertEqual(readCount, 3)
        XCTAssertEqual(store.displayedItem?.bodyMD, "Fresh replacement.")
        XCTAssertEqual(store.selected?.candidateHash, "sha256:original")
        XCTAssertTrue(store.decisionsDisabled)
        await store.decide(.approve, api: api, userID: "owner")
        let blockedRequests = await harness.requests
        XCTAssertTrue(blockedRequests.isEmpty)
        store.selectUpdated()
        await store.decide(.approve, api: api, userID: "owner")
        let sentRequests = await harness.requests
        let request = try XCTUnwrap(sentRequests.first)
        XCTAssertEqual(request.candidateHash, "sha256:replacement")
        XCTAssertEqual(request.runVersion, 7)
        XCTAssertEqual(request.expectedDecisionsVersion, 4)
        XCTAssertEqual(request.comment, "Keep my draft")
    }

    func testAccountChangeRejectsOldRefreshAndClearsOldSelectionAndDraft() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        store.drafts["item-1"] = .init(comment: "Private old draft", correction: "")
        let gate = ReviewAsyncGate()
        await harness.holdNextRead(gate)
        let oldRefresh = Task { await store.refresh(api: api, userID: "owner") }
        await gate.waitUntilSuspended()
        await harness.changeAccount()
        await harness.replace(try reviewFixture(itemOverrides: ["body_md": "Other account content."]))
        await store.refresh(api: api, userID: "other")
        await gate.release()
        await oldRefresh.value
        XCTAssertEqual(store.data?.items.first?.bodyMD, "Other account content.")
        XCTAssertNil(store.selected)
        XCTAssertTrue(store.drafts.isEmpty)
        XCTAssertNil(store.loadError)
        XCTAssertFalse(store.isRefreshing)
    }

    func testAccountChangeRejectsAnOldDecisionResponse() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        let gate = ReviewAsyncGate()
        await harness.holdNextWrite(gate)
        let oldDecision = Task { await store.decide(.approve, api: api, userID: "owner") }
        await gate.waitUntilSuspended()
        await harness.changeAccount()
        await harness.replace(try reviewFixture(itemOverrides: ["body_md": "Other account content."]))
        await store.refresh(api: api, userID: "other")
        await gate.release()
        await oldDecision.value
        XCTAssertEqual(store.data?.items.first?.bodyMD, "Other account content.")
        XCTAssertNil(store.pendingRequest)
        XCTAssertNil(store.decisionMessage)
        XCTAssertNil(store.decisionError)
        XCTAssertFalse(store.isSubmitting)
    }

    func testUncertainRetryUsesIdenticalRequestAndLocksOtherActions() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture(), failure: .network)
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        store.drafts["item-1"] = .init(comment: "Owner context", correction: "")
        await store.decide(.approve, api: api, userID: "owner")
        let original = try XCTUnwrap(store.pendingRequest)
        XCTAssertTrue(store.selectionLocked)
        await harness.replace(try reviewFixture(version: 4, itemOverrides: ["body_md": "Replacement while decision is unconfirmed.", "candidate_hash": "sha256:new"]))
        await store.refresh(api: api, userID: "owner")
        XCTAssertEqual(store.displayedItem?.bodyMD, "Replacement while decision is unconfirmed.")
        XCTAssertEqual(store.pendingRequest, original)
        await store.decide(.reject, api: api, userID: "owner")
        await store.retry(api: api, userID: "owner")
        let requests = await harness.requests
        XCTAssertEqual(requests, [original, original])
        XCTAssertEqual(requests[0].expectedDecisionsVersion, 3)
        XCTAssertEqual(requests[0].comment, "Owner context")
        XCTAssertNil(store.pendingRequest)
        XCTAssertEqual(store.decisionMessage, "Approved and held.")
        XCTAssertTrue(store.decisionsDisabled)
        XCTAssertTrue(store.hasReplacement, "An earlier receipt must remain distinct from the displayed replacement")
        let encoded = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(original)) as? [String: Any])
        XCTAssertEqual(encoded["run_version"] as? Int, 2)
        XCTAssertEqual(encoded["candidate_hash"] as? String, "sha256:original")
        XCTAssertNil(encoded["runVersion"])
    }

    func testForegroundNotificationSignalsAuthenticatedReviewRefreshWithoutNavigation() async {
        _ = PushRouteBuffer.shared.take()
        let completed = expectation(description: "presentation completed")
        let refreshed = expectation(description: "review refresh signaled")
        let observer = NotificationCenter.default.addObserver(forName: .brunnReviewRefresh, object: nil, queue: .main) { event in
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertNil(event.object)
            XCTAssertNil(PushRouteBuffer.shared.take())
            refreshed.fulfill()
        }
        defer { NotificationCenter.default.removeObserver(observer) }
        NotificationDelegateHandoff.finishPresentation([], route: .notification(notificationRef: "notification:11111111111111111111111111111111", deliveryRef: nil)) { _ in
            XCTAssertTrue(Thread.isMainThread)
            completed.fulfill()
        }
        await fulfillment(of: [completed, refreshed], timeout: 1)
    }

    func testConflictRequiresFreshReviewAndRetainsNote() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture(), failure: .conflict)
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        store.drafts["item-1"] = .init(comment: "Keep after conflict", correction: "")
        await store.decide(.defer, api: api, userID: "owner")
        XCTAssertTrue(store.conflict)
        XCTAssertTrue(store.decisionsDisabled)
        XCTAssertNil(store.pendingRequest)
        XCTAssertEqual(store.drafts["item-1"]?.comment, "Keep after conflict")
        store.selectUpdated()
        XCTAssertFalse(store.conflict)
    }

    func testAccountChangeAndReadonlyIdentityCannotSend() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        await harness.setIdentity(MeData(user: .init(id: "other", displayName: "Other"), capabilities: ["credential:manage", "save"]))
        await store.decide(.approve, api: api, userID: "owner")
        let requests = await harness.requests
        XCTAssertTrue(requests.isEmpty)
        XCTAssertFalse(store.canDecide)
        await harness.setIdentity(MeData(user: .init(id: "owner", displayName: "Owner"), capabilities: ["read"], readOnly: true))
        await store.refresh(api: api, userID: "owner")
        XCTAssertFalse(store.canDecide)
    }

    func testUnavailableSnapshotAndFailedRefreshDisableDecisions() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture(available: false))
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        XCTAssertTrue(store.decisionsDisabled)
        await harness.replace(try reviewFixture())
        await store.refresh(api: api, userID: "owner")
        XCTAssertFalse(store.decisionsDisabled)
        await harness.failReads()
        await store.refresh(api: api, userID: "owner")
        XCTAssertNotNil(store.data)
        XCTAssertNotNil(store.loadError)
        XCTAssertTrue(store.decisionsDisabled)
    }

    func testServerRedactionReplacesDisplayedEvidenceImmediately() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        await harness.replace(try reviewFixture(itemOverrides: ["stale": true, "reviewable": false,
                                                              "sources": [], "candidate": NSNull(),
                                                              "body_md": "Evidence unavailable."]))
        await store.refresh(api: api, userID: "owner")
        XCTAssertEqual(store.displayedItem?.bodyMD, "Evidence unavailable.")
        XCTAssertNil(store.displayedItem?.candidate)
        XCTAssertTrue(store.displayedItem?.sources.isEmpty == true)
        XCTAssertTrue(store.decisionsDisabled)
    }

    func testSessionRotationDuringIdentityProbeCannotSendOldDraft() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        await harness.rotateDuringNextIdentity()
        await store.decide(.approve, api: api, userID: "owner")
        let requests = await harness.requests
        XCTAssertTrue(requests.isEmpty)
        XCTAssertNotNil(store.decisionError)
    }

    func testPausedNullModeStillDecodesReview() throws {
        let original = try JSONEncoder().encode(reviewFixture())
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: original) as? [String: Any])
        json["mode"] = NSNull()
        json["paused"] = true
        let paused = try JSONDecoder().decode(DreamerReviewData.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertNil(paused.mode)
        XCTAssertTrue(paused.paused)
        XCTAssertEqual(paused.items.count, 1)
    }

    func testAPIDecisionRejectsChangedSessionAndFreezesCookieAndCSRF() async throws {
        let host = "review-" + UUID().uuidString.lowercased() + ".example"
        let base = URL(string: "https://\(host)/api/v1")!
        let jar = HTTPCookieStorage.shared
        func cookie(_ name: String, _ value: String) -> HTTPCookie {
            HTTPCookie(properties: [.name: name, .value: value, .domain: host, .path: "/", .secure: "TRUE"])!
        }
        let sessionA = cookie("__Host-brunn_session", "account-a")
        let csrfA = cookie("__Host-brunn_csrf", "csrf-a")
        let sessionB = cookie("__Host-brunn_session", "account-b")
        jar.setCookie(sessionA)
        jar.setCookie(csrfA)
        defer {
            jar.cookies(for: base)?.forEach(jar.deleteCookie)
            ReviewWireProtocol.handler = nil
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [ReviewWireProtocol.self]
        configuration.httpCookieStorage = jar
        let api = BrunnAPI(configuration: .init(baseURL: base),
                           session: URLSession(configuration: configuration), cookieStorage: jar)
        let loadedFingerprint = await api.authenticatedSessionFingerprint()
        let fingerprint = try XCTUnwrap(loadedFingerprint)
        let payload = DreamerDecisionRequest(itemID: "item-1", runEntryRef: "entry:run", runVersion: 2,
                                            candidateHash: "sha256:original", expectedDecisionsVersion: 3,
                                            decision: .approve, comment: nil, correction: nil, idempotencyKey: "test-key")
        jar.setCookie(sessionB)
        do {
            _ = try await api.dreamerReviewDecision(payload, expectedSessionFingerprint: fingerprint)
            XCTFail("A changed session must be refused before network work")
        } catch { XCTAssertEqual(error as? BrunnAPIError, .notConnected) }
        jar.setCookie(sessionA)
        ReviewWireProtocol.handler = { request in
            // Simulate cookie rotation after the API actor queued the request.
            jar.setCookie(sessionB)
            XCTAssertFalse(request.httpShouldHandleCookies)
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(request.url?.path, "/api/v1/dreamer/review/decisions")
            XCTAssertTrue(request.value(forHTTPHeaderField: "Cookie")?.contains("account-a") == true)
            XCTAssertFalse(request.value(forHTTPHeaderField: "Cookie")?.contains("account-b") == true)
            XCTAssertEqual(request.value(forHTTPHeaderField: "X-CSRF-Token"), "csrf-a")
            return Data(#"{"status":"complete","data":{"saved":true,"decision":"approve","application_status":"approved_held","message":"Approved. Application is held until publication is explicitly enabled.","state_version":4}}"#.utf8)
        }
        let result = try await api.dreamerReviewDecision(payload, expectedSessionFingerprint: fingerprint)
        XCTAssertEqual(result.decision, "approve")
        XCTAssertEqual(result.saved, true)
        XCTAssertEqual(result.applicationStatus, "approved_held")
    }

    func testLegacyResponseSeparatesNotesAndPreservesOlderServerCompatibility() throws {
        var snapshot = try reviewFixture()
        let note = try reviewFixture(itemOverrides: ["id": "old-note", "legacy": true]).items[0]
        snapshot.legacyItems = [note]
        let decoded = try JSONDecoder().decode(DreamerReviewData.self, from: JSONEncoder().encode(snapshot))
        XCTAssertEqual(decoded.reviewItems.map(\.id), ["item-1"])
        XCTAssertEqual(decoded.olderReports.map(\.id), ["old-note"])
        XCTAssertEqual(decoded.olderReports[0].legacy, true)
        XCTAssertEqual(decoded.counts.pending, 1)
        XCTAssertNil(try reviewFixture().legacyItems)

        let prose = try reviewFixture(itemOverrides: ["reviewable": false,
                                                     "candidate": ["body_md": "", "before_md": "", "after_md": ""]])
        XCTAssertEqual(prose.reviewItems.count, 1)
        XCTAssertTrue(prose.olderReports.isEmpty)
        XCTAssertFalse(prose.reviewItems[0].candidate?.hasContent ?? true)
        let temporarilyUnavailable = try reviewFixture(itemOverrides: ["reviewable": false, "candidate": NSNull(),
                                                                        "blocked_reason": "This older proposal describes work but has no generated candidate to approve."])
        XCTAssertEqual(temporarilyUnavailable.reviewItems.count, 1)
        XCTAssertTrue(temporarilyUnavailable.olderReports.isEmpty)
        let question = try reviewFixture(itemOverrides: ["kind": "question", "candidate": NSNull(), "reviewable": false])
        XCTAssertEqual(question.reviewItems.count, 1)
        let importedQuestion = try reviewFixture(itemOverrides: ["kind": "question", "candidate": NSNull(), "reviewable": false,
                                                                 "why_md": "Retained from an earlier run; a concrete candidate is required before application."])
        XCTAssertTrue(importedQuestion.reviewItems.isEmpty)
        XCTAssertEqual(importedQuestion.olderReports.count, 1)
    }

    func testHistoricalNotesCannotSendAnyDecision() async throws {
        var snapshot = try reviewFixture()
        let note = try reviewFixture(itemOverrides: ["id": "old-note", "legacy": true]).items[0]
        snapshot.legacyItems = [note]
        let harness = ReviewHarness(snapshot: snapshot)
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(note, historical: true)
        store.drafts[note.id] = .init(comment: "Context", correction: "Correction")
        XCTAssertTrue(store.selectionIsHistorical)
        XCTAssertTrue(store.decisionsDisabled)
        for action in [DreamerDecisionAction.approve, .reject, .defer, .correct] {
            await store.decide(action, api: api, userID: "owner")
        }
        let requests = await harness.requests
        XCTAssertTrue(requests.isEmpty)
    }

    func testActiveSelectionBecomingHistoricalImmediatelyLosesActions() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture())
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        await harness.replace(try reviewFixture(itemOverrides: ["legacy": true]))
        await store.refresh(api: api, userID: "owner")
        XCTAssertTrue(store.selectionIsHistorical)
        XCTAssertEqual(store.displayedItem?.legacy, true)
        XCTAssertTrue(store.decisionsDisabled)
        XCTAssertTrue(store.data?.reviewItems.isEmpty == true)
    }

    func testLegacyMembershipAndTitlePresentationDoNotRewriteOriginalText() throws {
        var snapshot = try reviewFixture()
        let item = snapshot.items[0]
        snapshot.legacyItems = [item]
        XCTAssertTrue(snapshot.reviewItems.isEmpty)
        XCTAssertEqual(snapshot.olderReports.map(\.id), [item.id])
        let note = try reviewFixture(itemOverrides: ["legacy": true, "title": "Applies next run unless vetoed: Earlier promise"]).items[0]
        XCTAssertEqual(note.historicalTitle, "Earlier promise")
        XCTAssertEqual(note.bodyMD, item.bodyMD)
        XCTAssertTrue(note.title.hasPrefix("Applies next run unless vetoed:"))
    }

    func testUncertainRequestCanFinishAfterHistoricalReclassification() async throws {
        let harness = ReviewHarness(snapshot: try reviewFixture(), failure: .network)
        let store = makeStore(harness)
        let api = BrunnAPI()
        await store.refresh(api: api, userID: "owner")
        store.select(try XCTUnwrap(store.data?.items.first))
        await store.decide(.approve, api: api, userID: "owner")
        let original = try XCTUnwrap(store.pendingRequest)
        await harness.replace(try reviewFixture(itemOverrides: ["legacy": true]))
        await store.refresh(api: api, userID: "owner")
        XCTAssertTrue(store.selectionIsHistorical)
        XCTAssertTrue(store.selectionLocked)
        await harness.setFailure(.conflict)
        await store.retry(api: api, userID: "owner")
        let requests = await harness.requests
        XCTAssertEqual(requests, [original, original])
        XCTAssertNil(store.pendingRequest)
        XCTAssertFalse(store.selectionLocked)
        XCTAssertTrue(store.decisionsDisabled)
    }

    private func makeStore(_ harness: ReviewHarness) -> DreamerReviewStore {
        DreamerReviewStore(loadReview: { _ in try await harness.read() },
                           loadIdentity: { _ in await harness.readIdentity() },
                           loadSessionFingerprint: { _ in await harness.fingerprint },
                           writeDecision: { _, request, _ in try await harness.write(request) })
    }
}

private enum ReviewFailure: Sendable { case network, conflict }
private actor ReviewHarness {
    var readCount = 0
    var readGate: ReviewAsyncGate?
    var writeGate: ReviewAsyncGate?
    var snapshot: DreamerReviewData
    var identity = MeData(user: .init(id: "owner", displayName: "Owner"), capabilities: ["credential:manage", "save"])
    var requests: [DreamerDecisionRequest] = []
    var fingerprint = "session-a"
    var rotateIdentity = false
    var failure: ReviewFailure?
    var readsFail = false
    init(snapshot: DreamerReviewData, failure: ReviewFailure? = nil) {
        self.snapshot = snapshot
        self.failure = failure
    }
    func replace(_ value: DreamerReviewData) { snapshot = value }
    func setFailure(_ value: ReviewFailure) { failure = value }
    func setIdentity(_ value: MeData) { identity = value }
    func failReads() { readsFail = true }
    func rotateDuringNextIdentity() { rotateIdentity = true }
    func holdNextRead(_ gate: ReviewAsyncGate) { readGate = gate }
    func holdNextWrite(_ gate: ReviewAsyncGate) { writeGate = gate }
    func changeAccount() {
        fingerprint = "session-other"
        identity = MeData(user: .init(id: "other", displayName: "Other"), capabilities: ["credential:manage", "save"])
    }
    func readIdentity() -> MeData {
        if rotateIdentity { fingerprint = "session-b"; rotateIdentity = false }
        return identity
    }
    func read() async throws -> DreamerReviewData {
        readCount += 1
        if readsFail { throw URLError(.notConnectedToInternet) }
        let captured = snapshot
        if let gate = readGate {
            readGate = nil
            await gate.suspend()
        }
        return captured
    }
    func write(_ request: DreamerDecisionRequest) async throws -> DreamerDecisionResult {
        requests.append(request)
        if let gate = writeGate {
            writeGate = nil
            await gate.suspend()
        }
        if let failure {
            self.failure = nil
            if failure == .network { throw URLError(.networkConnectionLost) }
            throw BrunnAPIError.server(status: 409, code: "dreamer_state_conflict", message: "Another decision was recorded.")
        }
        return DreamerDecisionResult(saved: true,
                                    decision: request.decision.rawValue,
                                    applicationStatus: "approved_held", message: "Approved and held.", stateVersion: 4)
    }
}

private actor ReviewAsyncGate {
    private var continuation: CheckedContinuation<Void, Never>?
    func suspend() async { await withCheckedContinuation { continuation = $0 } }
    func waitUntilSuspended() async {
        while continuation == nil { await Task.yield() }
    }
    func release() { continuation?.resume(); continuation = nil }
}

private func reviewFixture(version: Int = 3, available: Bool = true, itemOverrides: [String: Any] = [:]) throws -> DreamerReviewData {
    var item: [String: Any] = [
        "id": "item-1", "kind": "proposal", "title": "Full original proposal",
        "body_md": String(repeating: "Complete original question. ", count: 200),
        "run_id": "2026-09-08", "run_entry_ref": "entry:run", "run_version": 2,
        "candidate_hash": "sha256:original", "candidate": ["before_md": "Original.", "after_md": "Exact proposed content."],
        "sources": [["entry_ref": "entry:evidence", "version": 12, "excerpt": "Pinned evidence."]],
        "status": "pending", "reviewable": true, "stale": false,
    ]
    item.merge(itemOverrides) { _, new in new }
    let json: [String: Any] = ["available": available, "mode": "report-only", "paused": false,
                               "counts": ["pending": 1, "proposals": 1, "questions": 0, "approved_held": 0, "applied": 0],
                               "items": [item], "history": [], "decision_version": version]
    return try JSONDecoder().decode(DreamerReviewData.self, from: JSONSerialization.data(withJSONObject: json))
}

private final class ReviewWireProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> Data)?
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        guard let handler = Self.handler, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        let data = handler(request)
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}
