@testable import BrunnCore
import XCTest

final class AutomaticReviewPolicyTests: XCTestCase {
    func testAutomaticPolicyAndDeadlineDecodeWithoutAnOwnerApproval() throws {
        let json = #"""
        {"available":true,"mode":"full","paused":false,"auto_apply_after_hours":24,
         "counts":{"pending":1,"proposals":1,"questions":0,"approved_held":0,"applied":1},
         "items":[{"id":"one","kind":"proposal","title":"Update","body_md":"Proposed view",
          "run_id":"2026-09-12","run_entry_ref":"entry:run","run_version":1,"candidate_hash":"hash",
          "sources":[],"status":"pending","reviewable":true,"stale":false,"auto_apply_at":"2026-09-13T12:00:00.123456Z"}],
         "history":[{"id":"automatic:one","item_id":"older","decision":"auto_apply","at":"2026-09-12T12:00:00Z","application_status":"applied"}],
         "decision_version":4}
        """#
        let data = try JSONDecoder().decode(DreamerReviewData.self, from: Data(json.utf8))
        XCTAssertEqual(data.autoApplyAfterHours, 24)
        XCTAssertEqual(data.items.first?.autoApplyAt, "2026-09-13T12:00:00.123456Z")
        XCTAssertEqual(data.history.first?.decision, "auto_apply")
    }
}
