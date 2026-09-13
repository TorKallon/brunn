import XCTest
@testable import BrunnCore

final class TaskTimingContractTests: XCTestCase {
    func testInferredConsequenceCannotTurnAnOwnerDeadlineIntoAnUnconfirmedDeadline() {
        let candidate = AgentTaskCandidate(taskRef: "task", entryRef: "entry", version: 1, title: "Care", tier: 1,
            reason: "Serious consequence", provenanceMarkers: ["agent:aether"], hardDue: "2026-09-15T12:00:00Z", hardDueSource: "owner")
        XCTAssertTrue(candidate.hasInferredProvenance)
        XCTAssertFalse(candidate.hasInferredHardDeadline)
    }

    func testTomorrowDoesNotEncodeARollingDayOrMoveDueDate() throws {
        let data = try JSONEncoder().encode(AgentTaskUpdateOperation.tomorrow)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(json["type"] as? String, "snooze")
        XCTAssertEqual(json["tomorrow"] as? Bool, true)
        XCTAssertNil(json["days"])
        XCTAssertNil(json["hard_due"])
    }

    func testNewSourcedFieldsAcceptClearedValuesAndRetainNonemptyValues() throws {
        let cleared = #"{"id":"task","title":"Care","consequence":{"value":null,"source":"owner","set_at":"2026-09-12T00:00:00Z"},"recurrence":{"value":null,"source":"owner","set_at":"2026-09-12T00:00:00Z"}}"#
        let empty = try JSONDecoder().decode(AgentTaskDocument.self, from: Data(cleared.utf8))
        XCTAssertNil(empty.consequence)
        XCTAssertNil(empty.recurrence)
        let full = #"{"id":"task","title":"Care","consequence":{"value":{"description":"Loss of plants","severity":"serious","timing":{"kind":"after_due","days":2}},"source":"owner","set_at":"2026-09-12T00:00:00Z"},"recurrence":{"value":{"kind":"native","mode":"after_completion","every_days":14,"timezone":"UTC"},"source":"owner","set_at":"2026-09-12T00:00:00Z"}}"#
        let task = try JSONDecoder().decode(AgentTaskDocument.self, from: Data(full.utf8))
        XCTAssertEqual(task.consequence?.value.timing?.days, 2)
        XCTAssertEqual(task.recurrence?.value.mode, "after_completion")
        XCTAssertEqual(task.consequence?.source, "owner")
        let wrong = full.replacingOccurrences(of: #""every_days":14"#, with: #""every_days":"fourteen""#)
        XCTAssertThrowsError(try JSONDecoder().decode(AgentTaskDocument.self, from: Data(wrong.utf8)))
    }

    func testIndependentNextBudgetAndSeriousFourthItemEscapePreviewCap() {
        let urgent = (1...6).map { n in AgentTaskCandidate(taskRef: "urgent-\(n)", entryRef: "entry-\(n)", version: 1, title: "Urgent \(n)", tier: 1, reason: "Deadline", mustShow: n == 4) }
        let next = (1...8).map { n in AgentTaskCandidate(taskRef: "next-\(n)", entryRef: "entry-next-\(n)", version: 1, title: "Next \(n)", tier: 5, reason: "Ready") }
        let projection = AgentTaskTodayProjection.bounded(urgent: urgent, next: next, contextsAvailable: [])
        XCTAssertEqual(projection.next.count, 5)
        XCTAssertEqual(projection.attentionPreview().map(\.taskRef), ["urgent-1", "urgent-2", "urgent-3", "urgent-4"])
        XCTAssertEqual(projection.attentionPreview(expanded: true).count, 6)
    }
}
