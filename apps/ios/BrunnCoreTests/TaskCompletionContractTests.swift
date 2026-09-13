import Foundation
import XCTest
@testable import BrunnCore

final class TaskCompletionContractTests: XCTestCase {
    private let timestamp = "2026-09-12T22:00:00Z"

    private func cell(_ value: Any) -> [String: Any] {
        ["value": value, "source": "owner", "set_at": timestamp, "note": "Explicitly cleared"]
    }

    private func document(_ fields: [String: Any]) -> [String: Any] {
        ["id": "019f8800-0000-7000-8000-000000000001", "title": "Quick task"]
            .merging(fields) { _, new in new }
    }

    private func decode(_ fields: [String: Any]) throws -> AgentTaskDocument {
        try JSONDecoder().decode(AgentTaskDocument.self, from: JSONSerialization.data(withJSONObject: document(fields)))
    }

    func testCommittedCompletionWithClearedFieldsDecodes() throws {
        let task = document([
            "status": cell("done"), "ready_at": cell(NSNull()),
            "soft_due": cell(NSNull()), "hard_due": cell(NSNull()),
            "today_pin": cell(NSNull()), "estimate_minutes": cell(5),
        ])
        let detail: [String: Any] = [
            "task_ref": task["id"]!, "entry_ref": "entry:one", "version": 8,
            "title": "Quick task", "status": "done", "task": task,
            "created_at": timestamp, "updated_at": timestamp,
        ]
        let response: [String: Any] = ["status": "committed", "data": [
            "task": detail, "action": "complete", "done_today_count": 7,
            "correction_ref": NSNull(), "next_occurrence_task_ref": NSNull(), "replayed": false,
        ]]
        let result = try JSONDecoder().decode(WorkspaceEnvelope<AgentTaskUpdateData>.self,
            from: JSONSerialization.data(withJSONObject: response)).data
        XCTAssertEqual(result.task.status, .done)
        XCTAssertEqual(result.task.version, 8)
        XCTAssertEqual(result.doneTodayCount, 7)
        XCTAssertEqual(result.task.task.estimateMinutes?.value, 5)
        XCTAssertNil(result.task.task.readyAt)
        XCTAssertNil(result.task.task.softDue)
        XCTAssertNil(result.task.task.hardDue)
        XCTAssertNil(result.task.task.todayPin)
    }

    func testOptionalFieldsAcceptMissingNullAndExplicitlyClearedValues() throws {
        for fields in [[:], ["notes": NSNull()], [
            "notes": cell(NSNull()), "project": cell(NSNull()),
            "required_contexts": cell(NSNull()), "estimate_minutes": cell(NSNull()),
        ]] as [[String: Any]] {
            let task = try decode(fields)
            XCTAssertNil(task.notes)
            XCTAssertNil(task.project)
            XCTAssertNil(task.requiredContexts)
            XCTAssertNil(task.estimateMinutes)
        }
    }

    func testNonemptyFieldsRetainValuesAndProvenance() throws {
        let task = try decode([
            "notes": cell("Read this"), "hard_due": cell(timestamp),
            "required_contexts": cell(["phone"]), "estimate_minutes": cell(5),
        ])
        XCTAssertEqual(task.notes?.value, "Read this")
        XCTAssertEqual(task.notes?.source, "owner")
        XCTAssertEqual(task.notes?.setAt, timestamp)
        XCTAssertEqual(task.notes?.note, "Explicitly cleared")
        XCTAssertEqual(task.hardDue?.value, timestamp)
        XCTAssertEqual(task.requiredContexts?.value, ["phone"])
        XCTAssertEqual(task.estimateMinutes?.value, 5)
    }

    func testMalformedNonemptyFieldsAreNotSilentlyDiscarded() throws {
        for fields in [
            ["hard_due": cell(42)], ["estimate_minutes": cell("five")],
            ["required_contexts": cell("phone")], ["notes": ["value": "no provenance"]],
        ] as [[String: Any]] {
            XCTAssertThrowsError(try decode(fields))
        }
    }
}
