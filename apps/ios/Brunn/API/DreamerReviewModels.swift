import Foundation

public enum DreamerDecisionAction: String, Codable, Sendable {
    case approve, reject, `defer`, correct
}

public struct DreamerReviewSource: Codable, Sendable, Equatable {
    public let entryRef: String
    public let path: String?
    public let version: Int?
    public let label: String?
    public let excerpt: String?

    enum CodingKeys: String, CodingKey {
        case entryRef = "entry_ref"
        case path
        case version
        case label
        case excerpt
    }
}

public struct DreamerReviewCandidate: Codable, Sendable, Equatable {
    public let bodyMD: String?
    public let beforeMD: String?
    public let afterMD: String?
    public let targetPath: String?

    enum CodingKeys: String, CodingKey {
        case bodyMD = "body_md"
        case beforeMD = "before_md"
        case afterMD = "after_md"
        case targetPath = "target_path"
    }
}

public struct DreamerReviewItem: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let kind: String
    public let title: String
    public let bodyMD: String
    public let whyMD: String?
    public let uncertaintyMD: String?
    public let runID: String
    public let runEntryRef: String
    public let runVersion: Int
    public let candidateHash: String
    public let candidate: DreamerReviewCandidate?
    public let sources: [DreamerReviewSource]
    public let status: String
    public let reviewable: Bool
    public let stale: Bool
    public let blockedReason: String?

    enum CodingKeys: String, CodingKey {
        case id
        case kind
        case title
        case bodyMD = "body_md"
        case whyMD = "why_md"
        case uncertaintyMD = "uncertainty_md"
        case runID = "run_id"
        case runEntryRef = "run_entry_ref"
        case runVersion = "run_version"
        case candidateHash = "candidate_hash"
        case candidate
        case sources
        case status
        case reviewable
        case stale
        case blockedReason = "blocked_reason"
    }
}

public struct DreamerReviewDecision: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let itemID: String
    public let decision: String
    public let comment: String?
    public let correction: String?
    public let at: String
    public let applicationStatus: String

    enum CodingKeys: String, CodingKey {
        case id
        case itemID = "item_id"
        case decision
        case comment
        case correction
        case at
        case applicationStatus = "application_status"
    }
}

public struct DreamerReviewAttempt: Codable, Sendable, Equatable {
    public let date: String?
    public let outcome: String
    public let detail: String?
    public let startedAt: String?
    public let finishedAt: String?

    enum CodingKeys: String, CodingKey {
        case date
        case outcome
        case detail
        case startedAt = "started_at"
        case finishedAt = "finished_at"
    }
}

public struct DreamerReviewRun: Codable, Sendable, Equatable {
    public let runID: String
    public let entryRef: String
    public let version: Int
    public let at: String?

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case entryRef = "entry_ref"
        case version
        case at
    }
}

public struct DreamerReviewCounts: Codable, Sendable, Equatable {
    public let pending: Int
    public let proposals: Int
    public let questions: Int
    public let approvedHeld: Int
    public let applied: Int

    enum CodingKeys: String, CodingKey {
        case pending
        case proposals
        case questions
        case approvedHeld = "approved_held"
        case applied
    }
}

public struct DreamerReviewData: Codable, Sendable, Equatable {
    public let available: Bool
    public let mode: String?
    public let paused: Bool
    public let unavailableReason: String?
    public let lastAttempt: DreamerReviewAttempt?
    public let lastSuccessfulRun: DreamerReviewRun?
    public let counts: DreamerReviewCounts
    public let items: [DreamerReviewItem]
    public let history: [DreamerReviewDecision]
    public let decisionVersion: Int

    enum CodingKeys: String, CodingKey {
        case available
        case mode
        case paused
        case unavailableReason = "unavailable_reason"
        case lastAttempt = "last_attempt"
        case lastSuccessfulRun = "last_successful_run"
        case counts
        case items
        case history
        case decisionVersion = "decision_version"
    }
}

public struct DreamerDecisionRequest: Codable, Sendable, Equatable {
    public let itemID: String
    public let runEntryRef: String
    public let runVersion: Int
    public let candidateHash: String
    public let expectedDecisionsVersion: Int
    public let decision: DreamerDecisionAction
    public let comment: String?
    public let correction: String?
    public let idempotencyKey: String

    enum CodingKeys: String, CodingKey {
        case itemID = "item_id"
        case runEntryRef = "run_entry_ref"
        case runVersion = "run_version"
        case candidateHash = "candidate_hash"
        case expectedDecisionsVersion = "expected_decisions_version"
        case decision
        case comment
        case correction
        case idempotencyKey = "idempotency_key"
    }
}

public struct DreamerDecisionResult: Codable, Sendable, Equatable {
    public let saved: Bool?
    public let decision: DreamerReviewDecision
    public let applicationStatus: String
    public let message: String?
    public let stateVersion: Int?

    enum CodingKeys: String, CodingKey {
        case saved
        case decision
        case applicationStatus = "application_status"
        case message
        case stateVersion = "state_version"
    }
}

extension DreamerReviewItem {
    public var approvalBlock: String? {
        if stale { return "The supporting evidence has changed. A fresh candidate needs another review." }
        if status == "approved_held" { return "This candidate is already approved and held for authorized application." }
        if status == "needs_changes" { return "Your correction is awaiting a fresh candidate and another review." }
        if !reviewable { return blockedReason ?? "A concrete candidate and its evidence are needed before approval." }
        guard let candidate,
              [candidate.bodyMD, candidate.afterMD].compactMap({ $0 }).contains(where: {
                  !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
              }) else { return "There is no concrete candidate to approve yet." }
        if sources.isEmpty { return "Supporting sources are needed before approval." }
        return nil
    }
}
