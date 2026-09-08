import Foundation

@MainActor
final class DreamerReviewStore: ObservableObject {
    struct Draft {
        var comment = ""
        var correction = ""
    }

    @Published private(set) var data: DreamerReviewData?
    @Published private(set) var selected: DreamerReviewItem?
    @Published private(set) var selectedDecisionVersion = 0
    @Published private(set) var selectedFromOlderReports = false
    @Published private(set) var canDecide = false
    @Published private(set) var isRefreshing = false
    @Published private(set) var isSubmitting = false
    @Published private(set) var loadError: String?
    @Published private(set) var decisionError: String?
    @Published private(set) var decisionMessage: String?
    @Published private(set) var conflict = false
    @Published private(set) var pendingRequest: DreamerDecisionRequest?
    @Published var drafts: [String: Draft] = [:]

    private let loadReview: @Sendable (BrunnAPI) async throws -> DreamerReviewData
    private let loadIdentity: @Sendable (BrunnAPI) async throws -> MeData
    private let writeDecision: @Sendable (BrunnAPI, DreamerDecisionRequest, String) async throws -> DreamerDecisionResult
    private let loadSessionFingerprint: @Sendable (BrunnAPI) async -> String?
    private var ownerID: String?
    private var refreshUserID: String?
    private var refreshGeneration = 0
    private var refreshRequested = false

    init(
        loadReview: @escaping @Sendable (BrunnAPI) async throws -> DreamerReviewData = { try await $0.dreamerReview() },
        loadIdentity: @escaping @Sendable (BrunnAPI) async throws -> MeData = { try await $0.me() },
        loadSessionFingerprint: @escaping @Sendable (BrunnAPI) async -> String? = { $0.authenticatedSessionFingerprint() },
        writeDecision: @escaping @Sendable (BrunnAPI, DreamerDecisionRequest, String) async throws -> DreamerDecisionResult = { try await $0.dreamerReviewDecision($1, expectedSessionFingerprint: $2) }
    ) {
        self.loadReview = loadReview
        self.loadIdentity = loadIdentity
        self.writeDecision = writeDecision
        self.loadSessionFingerprint = loadSessionFingerprint
    }

#if DEBUG
    func loadUITestFixture() {
        if ProcessInfo.processInfo.arguments.contains("--ui-test-review-location") {
            loadUITestLocationFixture()
            return
        }
        let paragraphs = (1...6).map { number in
            "Detail \(number): The complete proposal keeps its original context. Read the supporting evidence and the uncertainty before recording an owner decision. This longer paragraph deliberately wraps across the phone screen."
        }
        let body = (paragraphs + ["Final detail: keep the plan in review until I decide."]).joined(separator: "\n\n")
        data = DreamerReviewData(
            available: true, mode: "report-only", paused: false, unavailableReason: nil,
            lastAttempt: nil, lastSuccessfulRun: nil,
            counts: .init(pending: 1, proposals: 0, questions: 1, approvedHeld: 0, applied: 0, legacy: 1),
            items: [.init(id: "demo-review-question", kind: "question",
                          title: "Should we keep this plan in review?", bodyMD: body,
                          whyMD: "The proposal needs a deliberate owner decision.",
                          uncertaintyMD: "The evidence does not establish whether the plan is complete.",
                          runID: "Demo run", runEntryRef: "entry:demo-review-run", runVersion: 2,
                          candidateHash: "sha256:demo", candidate: nil,
                          sources: [.init(entryRef: "entry:demo-review-source", path: nil, version: 12,
                                          label: "Evidence for the review", excerpt: "This evidence is pinned to version twelve.")],
                          status: "pending", reviewable: false, stale: false,
                          blockedReason: "This question needs your answer before a candidate can be prepared.")],
            history: [], decisionVersion: 0,
            legacyItems: [.init(id: "demo-review-legacy", kind: "proposal",
                                title: "Applies next run unless vetoed: Old promise from an earlier run", bodyMD: "I will prepare something useful next time. Applies next run unless vetoed.",
                                whyMD: "Retained from an earlier run; a concrete candidate is required before application.",
                                uncertaintyMD: nil, runID: "Earlier demo run", runEntryRef: "entry:demo-legacy-run", runVersion: 1,
                                candidateHash: "sha256:demo-legacy", candidate: .init(bodyMD: "", beforeMD: "", afterMD: "", targetPath: nil),
                                sources: [], status: "pending", reviewable: false, stale: false, blockedReason: nil, legacy: true)]
        )
        canDecide = false
        ownerID = nil
    }

    // Entirely fictional places, date, and observation windows.
    static let uiTestLocationSummary = """
    # February 3 · UTC

    | When | Where |
    | --- | --- |
    | About 06:11–08:16 | Station |
    | About 09:02–10:23 | Library |
    | About 10:41–10:44 | Cafe, brief stop |
    | About 11:08–16:32 | Park |
    | About 16:57–17:46 | Community center |
    | About 18:14–21:09 | Market |
    | About 21:27–21:28 | Station entrance |

    Ranges bracket observations; arrival and departure remain approximate.
    """

    private func loadUITestLocationFixture() {
        let replacement = ProcessInfo.processInfo.arguments.contains("--ui-test-review-replacement") && selected != nil
        let summary = replacement ? "# Updated example\n\nThe replacement summary arrived while this item was open." : Self.uiTestLocationSummary
        data = DreamerReviewData(
            available: true, mode: "report-only", paused: false, unavailableReason: nil,
            lastAttempt: nil, lastSuccessfulRun: nil,
            counts: .init(pending: 1, proposals: 1, questions: 0, approvedHeld: 0, applied: 0, legacy: 0),
            items: [.init(id: "demo-review-location", kind: "proposal",
                          title: "Where you were on February 3", bodyMD: "A summary of the day's observed places.",
                          whyMD: "", uncertaintyMD: "Ranges bracket observations; arrival and departure remain approximate.",
                          runID: "Demo run", runEntryRef: "entry:demo-review-run", runVersion: replacement ? 3 : 2,
                          candidateHash: replacement ? "sha256:demo-replacement" : "sha256:demo-location",
                          candidate: .init(bodyMD: summary, beforeMD: "Previous location summary.",
                                           afterMD: summary, targetPath: "derived/location/days/2040-02-03.md"),
                          sources: (1...30).map { number in
                              .init(entryRef: "entry:demo-location-evidence-\(number)", path: nil, version: 12,
                                    label: "Location evidence \(number)", excerpt: "Raw observation details \(number)")
                          },
                          status: "pending", reviewable: true, stale: false, blockedReason: nil, legacy: false)],
            history: [], decisionVersion: 0, legacyItems: []
        )
        canDecide = false
        ownerID = nil
    }
#endif

    var selectionIsHistorical: Bool {
        selectedFromOlderReports || selected?.isLegacyNote == true
            || data?.olderReports.contains(where: { $0.id == selected?.id }) == true
    }

    var displayedItem: DreamerReviewItem? {
        if let selected,
           let current = data?.allItems.first(where: { $0.id == selected.id }) {
            return current
        }
        return selected
    }

    var selectionLocked: Bool { isSubmitting || pendingRequest != nil }
    var hasReplacement: Bool {
        guard let selected, let current = data?.allItems.first(where: { $0.id == selected.id }) else { return false }
        return current.candidateHash != selected.candidateHash
            || current.runEntryRef != selected.runEntryRef || current.runVersion != selected.runVersion
    }
    var changed: Bool {
        guard let selected, let data else { return false }
        return data.decisionVersion != selectedDecisionVersion
            || data.allItems.first(where: { $0.id == selected.id }) != selected
    }
    var decisionsDisabled: Bool {
        selectionIsHistorical || !canDecide || data?.available != true || loadError != nil || changed
            || conflict || selectionLocked || decisionMessage != nil
    }

    func select(_ item: DreamerReviewItem, historical: Bool = false) {
        guard !selectionLocked, let data else { return }
        selected = item
        selectedFromOlderReports = historical || item.isLegacyNote
        selectedDecisionVersion = data.decisionVersion
        decisionMessage = nil
        decisionError = nil
        conflict = false
    }

    func selectUpdated() {
        guard let selected, let latest = data?.allItems.first(where: { $0.id == selected.id }) else { return }
        select(latest)
    }

    func refresh(api: BrunnAPI, userID: String?) async {
        // A new account invalidates every response still in flight, including a
        // refresh that was started by a view which has since been discarded.
        if refreshUserID != userID {
            refreshGeneration &+= 1
            refreshUserID = userID
            data = nil
            selected = nil
            selectedDecisionVersion = 0
            drafts = [:]
            pendingRequest = nil
            decisionMessage = nil
            decisionError = nil
            loadError = nil
            selectedFromOlderReports = false
            conflict = false
            ownerID = nil
            canDecide = false
            isRefreshing = false
            isSubmitting = false
            refreshRequested = false
        }
        guard let userID else { return }
        guard !isRefreshing else {
            refreshRequested = true
            return
        }
        let generation = refreshGeneration
        isRefreshing = true
        defer { if generation == refreshGeneration { isRefreshing = false } }
        repeat {
            refreshRequested = false
            do {
                guard let fingerprint = await loadSessionFingerprint(api) else { throw BrunnAPIError.notConnected }
                let identity = try await loadIdentity(api)
                guard identity.user.id == userID else { throw BrunnAPIError.notConnected }
                let snapshot = try await loadReview(api)
                guard await loadSessionFingerprint(api) == fingerprint else { throw BrunnAPIError.notConnected }
                guard generation == refreshGeneration else { return }
                ownerID = identity.user.id
                canDecide = identity.capabilities.contains("credential:manage") && identity.capabilities.contains("save") && !identity.readOnly
                data = snapshot
                loadError = nil
            } catch {
                guard generation == refreshGeneration else { return }
                canDecide = false
                loadError = error.localizedDescription
            }
        } while refreshRequested
    }

    func decide(_ action: DreamerDecisionAction, api: BrunnAPI, userID: String?) async {
        guard !decisionsDisabled, let selected else { return }
        guard action != .approve || selected.approvalBlock == nil else { return }
        let draft = drafts[selected.id] ?? Draft()
        let comment = draft.comment.trimmingCharacters(in: .whitespacesAndNewlines)
        let correction = draft.correction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard comment.count <= 4000, correction.count <= 4000,
              action != .correct || !correction.isEmpty else { return }
        let request = DreamerDecisionRequest(
            itemID: selected.id, runEntryRef: selected.runEntryRef,
            runVersion: selected.runVersion, candidateHash: selected.candidateHash,
            expectedDecisionsVersion: selectedDecisionVersion, decision: action,
            comment: comment.isEmpty ? nil : comment,
            correction: action == .correct ? correction : nil,
            idempotencyKey: "ios_dreamer_decision_" + UUID().uuidString.lowercased()
        )
        await submit(request, api: api, userID: userID)
    }

    func retry(api: BrunnAPI, userID: String?) async {
        guard !isSubmitting, let pendingRequest else { return }
        await submit(pendingRequest, api: api, userID: userID)
    }

    private func submit(_ request: DreamerDecisionRequest, api: BrunnAPI, userID: String?) async {
        guard ownerID == userID, userID != nil else { return }
        let generation = refreshGeneration
        isSubmitting = true
        decisionError = nil
        defer { if generation == refreshGeneration { isSubmitting = false } }
        // Bind each attempt to one current session. A same-owner sign-in can
        // retry the retained request, but an identity probe and POST can never
        // span two sessions.
        guard let expectedFingerprint = await loadSessionFingerprint(api) else {
            guard generation == refreshGeneration else { return }
            canDecide = false
            decisionError = BrunnAPIError.notConnected.localizedDescription
            return
        }
        // Recheck the interactive account before every send, including exact retries.
        do {
            let identity = try await loadIdentity(api)
            guard generation == refreshGeneration else { return }
            guard identity.user.id == userID, !identity.readOnly,
                  identity.capabilities.contains("credential:manage"), identity.capabilities.contains("save"),
                  await loadSessionFingerprint(api) == expectedFingerprint
            else { throw BrunnAPIError.notConnected }
        } catch {
            guard generation == refreshGeneration else { return }
            canDecide = false
            decisionError = error.localizedDescription
            return
        }
        guard generation == refreshGeneration else { return }
        pendingRequest = request
        do {
            let result = try await writeDecision(api, request, expectedFingerprint)
            guard generation == refreshGeneration else { return }
            pendingRequest = nil
            decisionMessage = result.message ?? result.applicationStatus.replacingOccurrences(of: "_", with: " ")
            await refresh(api: api, userID: userID)
        } catch {
            guard generation == refreshGeneration else { return }
            decisionError = error.localizedDescription
            if let apiError = error as? BrunnAPIError, apiError == .notConnected {
                pendingRequest = nil
                canDecide = false
            }
            if case let BrunnAPIError.server(status, _, _) = error, (400..<500).contains(status) {
                pendingRequest = nil
                if status == 409 {
                    conflict = true
                    await refresh(api: api, userID: userID)
                }
            }
            // Transport/server/decoding failures retain the exact request and key.
            // Selecting another item or editing the draft stays locked until retry.
        }
    }
}
