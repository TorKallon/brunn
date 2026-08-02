@testable import StraylightCore
import XCTest

final class CoreContractTests: XCTestCase {
    func testStructuredBriefingDecodesCurrentWorkspaceEnvelope() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "path": "Briefings/2026/Morning briefing - 2026-08-02.md",
            "entry_ref": "entry:morning",
            "version": 2,
            "current_version": 2,
            "date": "2026-08-02",
            "edition": "morning",
            "briefing": {
              "schema": "briefing.v1",
              "date": "2026-08-02",
              "edition": "morning",
              "generated_at": "2026-08-02T06:30:00-07:00",
              "summary_md": ["**One** useful line."],
              "sections": [{
                "topic": "straylight",
                "title": "Straylight",
                "items": [{
                  "id": "native-ios",
                  "kind": "project",
                  "headline_md": "**Native iOS is active.**",
                  "delta": "update",
                  "story": {"key": "native-ios", "urls": ["https://example.com"]}
                }]
              }],
              "delta": {"added": [], "changed": ["native-ios"], "removed": []}
            },
            "markdown": "# Morning briefing",
            "created_at": "2026-08-02T06:30:00-07:00",
            "versions": [
              {"version": 1, "created_at": "2026-08-02T06:30:00-07:00"},
              {"version": 2, "created_at": "2026-08-02T10:00:00-07:00"}
            ],
            "workspace_generation": 42
          }
        }
        """#.data(using: .utf8)!

        let decoder = JSONDecoder()
        let envelope = try decoder.decode(
            WorkspaceEnvelope<BriefingEditionData>.self,
            from: json
        )

        XCTAssertEqual(envelope.status, "complete")
        XCTAssertEqual(envelope.data.entryRef, "entry:morning")
        XCTAssertEqual(envelope.data.briefing?.summaryMD?.first, "**One** useful line.")
        XCTAssertEqual(envelope.data.briefing?.sections?.first?.items.first?.headlineMD, "**Native iOS is active.**")
        XCTAssertEqual(envelope.data.versions.count, 2)
    }

    func testStructuredBriefingPreservesEverySummarySectionItemAndRevision() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "path": "Briefings/2026/Morning briefing - 2026-08-02.md",
            "entry_ref": "entry:morning",
            "version": 3,
            "current_version": 3,
            "date": "2026-08-02",
            "edition": "morning",
            "briefing": {
              "schema": "briefing.v1",
              "date": "2026-08-02",
              "edition": "morning",
              "timezone": "America/Los_Angeles",
              "generated_at": "2026-08-02T06:30:00-07:00",
              "summary_md": [
                "Summary one",
                "Summary two",
                "Summary three",
                "Summary four",
                "Summary five"
              ],
              "sections": [
                {
                  "topic": "projects",
                  "title": "Projects",
                  "items": [
                    {
                      "id": "project-new",
                      "kind": "project",
                      "headline_md": "A new project item",
                      "body_md": "Project body",
                      "why_it_matters": "Project impact",
                      "delta": "new",
                      "story": {
                        "key": "project-new",
                        "urls": ["https://example.com/project"]
                      }
                    },
                    {
                      "id": "project-update",
                      "kind": "project",
                      "headline_md": "An updated project item",
                      "detail_md": "Expanded project detail",
                      "what_changed": "A material number changed.",
                      "delta": "update",
                      "story": {"key": "project-update"}
                    }
                  ]
                },
                {
                  "topic": "news",
                  "title": "News",
                  "items": [
                    {
                      "id": "news-correction",
                      "kind": "news",
                      "headline_md": "A corrected news item",
                      "body_md": "Corrected context",
                      "what_changed": "The earlier report was corrected.",
                      "delta": "update",
                      "story": {
                        "key": "news-correction",
                        "urls": ["https://example.com/correction"]
                      }
                    }
                  ]
                },
                {
                  "topic": "metrics",
                  "title": "Metrics",
                  "items": [
                    {
                      "id": "metric-seen",
                      "kind": "metric",
                      "headline_md": "A corroborated metric",
                      "delta": "corroboration",
                      "story": {"key": "metric-seen"}
                    }
                  ]
                }
              ],
              "delta": {
                "added": ["project-new"],
                "changed": ["project-update", "news-correction"],
                "removed": ["retired-item"]
              }
            },
            "markdown": "# Morning briefing",
            "created_at": "2026-08-02T06:30:00-07:00",
            "versions": [
              {"version": 1, "created_at": "2026-08-02T06:30:00-07:00"},
              {"version": 2, "created_at": "2026-08-02T10:00:00-07:00"},
              {"version": 3, "created_at": "2026-08-02T14:00:00-07:00"}
            ],
            "workspace_generation": 42
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<BriefingEditionData>.self,
            from: json
        )
        let payload = try XCTUnwrap(envelope.data.briefing)

        XCTAssertEqual(payload.summaryMD?.count, 5)
        XCTAssertEqual(payload.sections?.map(\.topic), ["projects", "news", "metrics"])
        XCTAssertEqual(
            payload.sections?.flatMap(\.items).map(\.id),
            ["project-new", "project-update", "news-correction", "metric-seen"]
        )
        XCTAssertEqual(
            payload.sections?.flatMap(\.items).first(where: { $0.id == "project-new" })?.story?.urls,
            ["https://example.com/project"]
        )
        XCTAssertEqual(
            payload.sections?.flatMap(\.items).first(where: { $0.id == "news-correction" })?.whatChanged,
            "The earlier report was corrected."
        )
        XCTAssertEqual(payload.delta?.changed, ["project-update", "news-correction"])
        XCTAssertEqual(envelope.data.versions.map(\.version), [1, 2, 3])
    }

    func testSearchCandidateAcceptsBudgetedTextWithoutExcerpt() throws {
        let json = #"""
        {
          "reference": "entry:one",
          "path": "sources/One.md",
          "title": "One",
          "representation": "selected_source_section",
          "text": "Bounded hydrated text"
        }
        """#.data(using: .utf8)!
        let candidate = try JSONDecoder().decode(WorkspaceSearchCandidate.self, from: json)
        XCTAssertEqual(candidate.previewText, "Bounded hydrated text")
    }

    func testExactReadDecodesDeployedItemWithoutPerItemStatus() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "workspace_generation": 42,
            "items": [{
              "reference": "entry:one",
              "path": "sources/One.md",
              "title": "One",
              "version": 3,
              "content_hash": "sha256:abc",
              "media_type": "text/markdown",
              "view": "full",
              "text": "# Exact source",
              "updated_at": "2026-08-02T12:00:00Z"
            }]
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<WorkspaceReadData>.self,
            from: json
        )

        XCTAssertEqual(envelope.status, "complete")
        XCTAssertEqual(envelope.data.items.first?.text, "# Exact source")
    }

    func testExactReadDecodesDeployedNotFoundItem() throws {
        let json = #"""
        {
          "status": "partial",
          "data": {
            "workspace_generation": 42,
            "items": [{
              "status": "not_found",
              "path": "sources/Missing.md",
              "reference": "entry:missing",
              "error": {
                "code": "entry_not_found",
                "message": "The source no longer exists."
              }
            }]
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<WorkspaceReadData>.self,
            from: json
        )

        XCTAssertEqual(envelope.data.items.first?.status, "not_found")
        XCTAssertEqual(envelope.data.items.first?.error?.code, "entry_not_found")
    }

    func testExactReadRequestPinsSearchCandidateVersion() throws {
        let request = ReadRequest(
            requests: [ReadRequestItem(reference: "entry:one", version: 7)]
        )
        let data = try JSONEncoder().encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let requests = try XCTUnwrap(object["requests"] as? [[String: Any]])

        XCTAssertEqual(requests.first?["version"] as? Int, 7)
    }

    func testTypedDeepLinkAcceptsKnownBriefingRoute() throws {
        let url = try XCTUnwrap(URL(string: "straylight://briefing/2026-08-02/morning?item=native-ios"))
        XCTAssertEqual(
            AppRoute(url: url),
            .briefing(date: "2026-08-02", edition: "morning", itemID: "native-ios")
        )
    }

    func testTypedDeepLinkRejectsArbitraryWebURL() throws {
        let url = try XCTUnwrap(URL(string: "https://example.com/briefing/2026-08-02/morning"))
        XCTAssertNil(AppRoute(url: url))
    }

    func testBriefingPathMatchesDeployedRoute() {
        XCTAssertEqual(
            StraylightAPI.briefingPath(date: "2026-08-02", edition: "morning"),
            "workspace/briefings/2026-08-02/morning"
        )
    }

    func testBriefingListDecodesPaginationCursorWithoutLosingRowMetadata() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "editions": [{
              "date": "2026-08-02",
              "edition": "morning",
              "path": "Briefings/2026/Morning briefing - 2026-08-02.md",
              "entry_ref": "entry:morning",
              "version": 3,
              "generated_at": "2026-08-02T06:30:00-07:00",
              "summary_md": ["One", "Two"],
              "section_titles": ["Projects", "News"],
              "item_count": 4
            }],
            "limit": 30,
            "truncated": true,
            "next": {
              "after_path": "Briefings/2026/Morning briefing - 2026-07-04.md"
            },
            "workspace_generation": 77
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<BriefingListData>.self,
            from: json
        )

        XCTAssertTrue(envelope.data.truncated)
        XCTAssertEqual(
            envelope.data.next?.afterPath,
            "Briefings/2026/Morning briefing - 2026-07-04.md"
        )
        XCTAssertEqual(envelope.data.editions.first?.summaryMD, ["One", "Two"])
        XCTAssertEqual(envelope.data.editions.first?.sectionTitles, ["Projects", "News"])
        XCTAssertEqual(envelope.data.editions.first?.itemCount, 4)
    }

    func testTopicsSnapshotDecodesTruncationAndPendingRequestSignals() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "topics": [{
              "slug": "straylight",
              "name": "Straylight",
              "section_order": 10,
              "mode": "every_briefing",
              "editions": ["morning", "evening"],
              "schedule": "06:30",
              "entities": ["Straylight"],
              "symbols": ["STRAY"],
              "suppress_unchanged": true,
              "freshness_hours": 12,
              "body": "Product and reliability updates.",
              "truncated": true,
              "parse_error": "The trailing instructions were truncated.",
              "path": "Briefings/Topics/straylight.md",
              "entry_ref": "entry:topic-straylight",
              "version": 4
            }],
            "pending_requests": [{
              "path": "Briefings/Requests/2026-08-02 - native-ios.md",
              "entry_ref": "entry:request-native-ios",
              "date": "2026-08-02",
              "item_id": "native-ios",
              "edition_ref": "entry:morning",
              "topic": "straylight",
              "note": "Go deeper on the mobile contract."
            }],
            "pending_requests_truncated": true,
            "feedback_path": "Briefings/Feedback/2026-08.md",
            "feedback_tail": ["- useful"],
            "workspace_generation": 78
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<BriefingTopicsSnapshot>.self,
            from: json
        )
        let topic = try XCTUnwrap(envelope.data.topics.first)
        let pending = try XCTUnwrap(envelope.data.pendingRequests.first)

        XCTAssertEqual(topic.sectionOrder, 10)
        XCTAssertEqual(topic.suppressUnchanged, true)
        XCTAssertEqual(topic.freshnessHours, 12)
        XCTAssertEqual(topic.truncated, true)
        XCTAssertEqual(topic.parseError, "The trailing instructions were truncated.")
        XCTAssertEqual(pending.itemID, "native-ios")
        XCTAssertEqual(pending.editionRef, "entry:morning")
        XCTAssertEqual(envelope.data.pendingRequestsTruncated, true)
        XCTAssertEqual(envelope.data.feedbackTail, ["- useful"])
    }

    func testBriefingItemActionRequestEncodesDeployedSnakeCaseKeys() throws {
        let request = BriefingItemActionRequest(
            action: "expand",
            editionRef: "entry:morning",
            itemID: "native-ios",
            topicSlug: "straylight",
            verdict: "follow_closer",
            note: "Go deeper."
        )

        let data = try JSONEncoder().encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(object["action"] as? String, "expand")
        XCTAssertEqual(object["edition_ref"] as? String, "entry:morning")
        XCTAssertEqual(object["item_id"] as? String, "native-ios")
        XCTAssertEqual(object["topic_slug"] as? String, "straylight")
        XCTAssertEqual(object["verdict"] as? String, "follow_closer")
        XCTAssertEqual(object["note"] as? String, "Go deeper.")
        XCTAssertNil(object["editionRef"])
        XCTAssertNil(object["itemID"])
        XCTAssertNil(object["topicSlug"])
    }
}
