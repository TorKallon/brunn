@testable import BrunnCore
import XCTest

final class CoreContractTests: XCTestCase {
    func testDashboardDecodesStorageActivityAndAccessWithoutSecrets() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "generated_at": "2026-08-02T23:00:00Z",
            "timezone": "America/Los_Angeles",
            "workspace_generation": 84,
            "activity_tracking_started_at": "2026-08-02T20:00:00Z",
            "tracking": {
              "status": "enabled",
              "dropped_events": 0,
              "flush_failures": 0
            },
            "storage": {
              "text": {"count": 12482, "size_bytes": 19428821},
              "binary": {
                "count": 418,
                "size_bytes": 2874102394,
                "semantics": "physical_object_versions",
                "status": "fresh",
                "observed_at": "2026-08-02T22:59:00Z"
              }
            },
            "today": {
              "read_operations": 187,
              "read_bytes": 9412201,
              "write_operations": 23,
              "write_bytes": 1084002
            },
            "activity": [{
              "date": "2026-08-02",
              "period_start": "2026-08-02T07:00:00Z",
              "period_end": "2026-08-03T07:00:00Z",
              "read_operations": 187,
              "read_bytes": 9412201,
              "write_operations": 23,
              "write_bytes": 1084002
            }],
            "access": [{
              "id": "credential:ios",
              "name": "iPhone",
              "kind": "api_credential",
              "manageable": true,
              "access": "read_only",
              "status": "active",
              "scope_ids": ["scope:root"],
              "capabilities": ["query", "read", "status"],
              "last_used_at": "2026-08-02T22:58:00Z",
              "last_operation": "workspace.read",
              "read_operations_today": 12,
              "write_operations_today": 0
            }],
            "coverage": {"days": 7, "activity": "tracked_operations_only"}
          }
        }
        """#.data(using: .utf8)!

        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<WorkspaceDashboardData>.self,
            from: json
        )

        XCTAssertEqual(envelope.data.storage.text.count, 12_482)
        XCTAssertEqual(envelope.data.storage.binary.sizeBytes, 2_874_102_394)
        XCTAssertEqual(envelope.data.activity.first?.readOperations, 187)
        XCTAssertEqual(envelope.data.access.first?.name, "iPhone")
        XCTAssertEqual(envelope.data.access.first?.lastOperation, "workspace.read")
        XCTAssertEqual(envelope.data.tracking?.status, "enabled")
        XCTAssertEqual(envelope.data.coverage?.days, 7)
    }

    func testDreamingStatusDecodesAccountAndWeeklyUsage() throws {
        let json = """
        {"control":{"enabled":true,"mode":"report-only"},
         "dreamer":{"runtime":{"account":"acct_123","account_email":"dreamer@example.com","plan":"pro",
           "last_attempt_date":"2026-09-10","last_attempt_result":"partial",
           "usage":{"observed_at":"2026-09-11T09:01:00Z","email":"dreamer@example.com","plan_type":"pro",
             "primary":{"used_percent":42.4,"window_minutes":10080,"resets_at":"2026-09-16T04:02:42Z"},
             "secondary":null}},
           "schedule":{"hour":2,"timezone":"America/Los_Angeles","time_budget_seconds":10800}}}
        """
        let status = try JSONDecoder().decode(DreamingStatusData.self, from: Data(json.utf8))
        XCTAssertEqual(status.accountLabel, "dreamer@example.com")
        XCTAssertEqual(status.planLabel, "pro")
        XCTAssertEqual(status.dreamer?.runtime?.usage?.weekly?.usedPercent, 42.4)
        XCTAssertEqual(status.dreamer?.runtime?.usage?.weekly?.windowMinutes, 10080)
        XCTAssertEqual(status.schedule?.hour, 2)
        XCTAssertFalse(json.contains("token"))
    }

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
              "sections": [{
                "topic": "brunn",
                "title": "Brunn",
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
        XCTAssertEqual(envelope.data.briefing?.sections?.first?.items.first?.headlineMD, "**Native iOS is active.**")
        XCTAssertEqual(envelope.data.versions.count, 2)
    }

    func testStructuredBriefingPreservesEverySectionItemAndRevision() throws {
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
          "text": "Bounded hydrated text",
          "score": 9.75,
          "updated_at": "2026-08-03T10:15:00Z"
        }
        """#.data(using: .utf8)!
        let candidate = try JSONDecoder().decode(WorkspaceSearchCandidate.self, from: json)
        XCTAssertEqual(candidate.previewText, "Bounded hydrated text")
        XCTAssertEqual(candidate.score, 9.75)
        XCTAssertEqual(candidate.updatedAt, "2026-08-03T10:15:00Z")
    }

    func testSearchRequestEncodesSelectedServerSort() throws {
        let request = SearchRequest(
            queries: [SearchQuery(query: "project state", sort: .lastModified)]
        )
        let data = try JSONEncoder().encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let queries = try XCTUnwrap(object["queries"] as? [[String: Any]])

        XCTAssertEqual(queries.first?["sort"] as? String, "last_modified")
    }

    func testLegacySearchOrderingUsesModifiedDateForEqualRelevance() {
        let older = WorkspaceSearchCandidate(
            path: "sources/Older.md",
            title: "Older",
            score: 8,
            updatedAt: "2026-08-01T12:00:00Z"
        )
        let newer = WorkspaceSearchCandidate(
            path: "sources/Newer.md",
            title: "Newer",
            score: 8,
            updatedAt: "2026-08-02T12:00:00Z"
        )
        let strongest = WorkspaceSearchCandidate(
            path: "sources/Strongest.md",
            title: "Strongest",
            score: 12,
            updatedAt: "2026-07-01T12:00:00Z"
        )

        XCTAssertEqual(
            WorkspaceSearchOrdering.sorted([older, newer, strongest], by: .bestMatch).map(\.title),
            ["Strongest", "Newer", "Older"]
        )
        XCTAssertEqual(
            WorkspaceSearchOrdering.sorted([older, strongest, newer], by: .lastModified).map(\.title),
            ["Newer", "Older", "Strongest"]
        )
    }

    func testWikiEntryLinkResolvesHostedVaultRootAndRelativeMarkdownPaths() throws {
        let wiki = try XCTUnwrap(WorkspaceEntryLink(
            target: "Topics/Gaming/Gaming#Current|Ignored alias",
            label: "Gaming",
            isWikiLink: true
        ))
        XCTAssertTrue(
            wiki.pathCandidates(relativeTo: "sources/General Space/Current note.md")
                .contains("sources/Topics/Gaming/Gaming.md")
        )
        XCTAssertEqual(wiki.lookupTerm, "Gaming")

        let sibling = try XCTUnwrap(WorkspaceEntryLink(
            target: "Sibling",
            isWikiLink: true
        ))
        XCTAssertEqual(
            sibling.pathCandidates(relativeTo: "sources/General Space/Current note.md").first,
            "sources/General Space/Sibling.md"
        )
        XCTAssertEqual(
            WorkspaceEntryRequest(
                link: sibling,
                sourcePath: "sources/General Space/Current note.md"
            ).lookupTerm,
            "Sibling"
        )

        let vaultRoot = try XCTUnwrap(WorkspaceEntryLink(
            target: "Projects/Plan",
            isWikiLink: true
        ))
        XCTAssertEqual(
            vaultRoot.pathCandidates(relativeTo: "sources/General Space/Current note.md").first,
            "Projects/Plan.md"
        )
        XCTAssertNil(WorkspaceEntryRequest(
            link: vaultRoot,
            sourcePath: "sources/General Space/Current note.md"
        ).lookupTerm)

        let canonical = try XCTUnwrap(WorkspaceEntryLink(
            target: "sources/Projects/Plan.md",
            isWikiLink: true
        ))
        XCTAssertEqual(
            canonical.pathCandidates(relativeTo: "sources/General Space/Current note.md").first,
            "sources/Projects/Plan.md"
        )

        let relative = try XCTUnwrap(WorkspaceEntryLink(target: "../Other note.md"))
        XCTAssertEqual(
            relative.pathCandidates(relativeTo: "sources/General Space/Current note.md").first,
            "sources/Other note.md"
        )

        let reference = try XCTUnwrap(WorkspaceEntryLink(
            target: "entry:11111111-1111-1111-1111-111111111111#Details"
        ))
        XCTAssertEqual(reference.reference, "entry:11111111-1111-1111-1111-111111111111")

        let queried = try XCTUnwrap(WorkspaceEntryLink(target: "Sibling.md?view=compact#Details"))
        XCTAssertEqual(
            queried.pathCandidates(relativeTo: "sources/General Space/Current note.md").first,
            "sources/General Space/Sibling.md"
        )
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

    func testEntryLinkReadRequestEncodesServerConfirmedUniqueTarget() throws {
        let request = ReadRequest(
            requests: [ReadRequestItem(linkTarget: "Roadmap")]
        )
        let data = try JSONEncoder().encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let requests = try XCTUnwrap(object["requests"] as? [[String: Any]])

        XCTAssertEqual(requests.first?["link_target"] as? String, "Roadmap")
        XCTAssertNil(requests.first?["path"])
        XCTAssertNil(requests.first?["ref"])
    }

    func testTypedDeepLinkAcceptsKnownBriefingRoute() throws {
        let url = try XCTUnwrap(URL(string: "brunn://briefing/2026-08-02/morning?item=native-ios"))
        XCTAssertEqual(
            AppRoute(url: url),
            .briefing(date: "2026-08-02", edition: "morning", itemID: "native-ios")
        )
    }

    func testTypedNotificationRouteRequiresLowercaseOpaqueIdentifiers() throws {
        let notificationID = "abcdefabcdefabcdefabcdefabcdefab"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let url = try XCTUnwrap(URL(
            string: "brunn://notification/\(notificationID)?delivery=\(deliveryID)"
        ))

        XCTAssertEqual(
            AppRoute(url: url),
            .notification(
                notificationRef: "notification:\(notificationID)",
                deliveryRef: "delivery:\(deliveryID)"
            )
        )
        XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(
            string: "brunn://notification/\(notificationID.uppercased())?delivery=\(deliveryID)"
        ))))
        XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(
            string: "brunn://notification/short?delivery=\(deliveryID)"
        ))))
    }

    func testNotificationListDecodesExactSourceTargetAndDeliveryContract() throws {
        let json = #"""
        {
          "items": [{
            "notification_ref": "notification:11111111111111111111111111111111",
            "kind": "briefing_ready",
            "importance": "important",
            "title": "Morning briefing ready",
            "body": "Private durable detail.",
            "source": {
              "type": "entry",
              "ref": "entry:morning",
              "version_ref": "version:morning-v3"
            },
            "target": {
              "type": "briefing",
              "date": "2026-08-02",
              "edition": "morning",
              "item_id": "native-ios"
            },
            "occurred_at": "2026-08-02T06:30:00-07:00",
            "opened_at": null,
            "acknowledged_at": null,
            "deliveries": [{
              "delivery_ref": "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "state": "accepted_by_apns",
              "accepted_at": "2026-08-02T06:30:02-07:00"
            }, {
              "delivery_ref": "delivery:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "state": "suppressed",
              "last_error_code": "transport_disabled"
            }]
          }],
          "next_cursor": "cursor:opaque",
          "unread_count": 4
        }
        """#.data(using: .utf8)!

        let response = try JSONDecoder().decode(NotificationListResponse.self, from: json)
        let notification = try XCTUnwrap(response.items.first)

        XCTAssertEqual(response.unreadCount, 4)
        XCTAssertEqual(response.nextCursor, "cursor:opaque")
        XCTAssertEqual(notification.source?.reference, "entry:morning")
        XCTAssertEqual(notification.source?.versionRef, "version:morning-v3")
        XCTAssertEqual(notification.target.type, .briefing)
        XCTAssertEqual(notification.target.itemID, "native-ios")
        XCTAssertEqual(notification.deliveries.map(\.state), [.acceptedByAPNs, .suppressed])
        XCTAssertEqual(notification.deliveries.last?.lastErrorCode, "transport_disabled")
        XCTAssertTrue(notification.isUnread)
    }

    func testNotificationMutationRequestsEncodeExactWireKeys() throws {
        let receipt = NotificationReceiptRequest(
            kind: .opened,
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        let receiptObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(receipt)) as? [String: Any]
        )
        XCTAssertEqual(receiptObject["kind"] as? String, "opened")
        XCTAssertEqual(
            receiptObject["delivery_ref"] as? String,
            "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        XCTAssertNil(receiptObject["receipt_type"])

        let installation = NotificationInstallationRequest(
            environment: "development",
            appID: "com.rourkem.brunn",
            deviceToken: "00ff"
        )
        let installationObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(installation)) as? [String: Any]
        )
        XCTAssertEqual(installationObject["platform"] as? String, "ios")
        XCTAssertEqual(installationObject["app_id"] as? String, "com.rourkem.brunn")
        XCTAssertEqual(installationObject["preview"] as? String, "generic")
        XCTAssertEqual(installationObject["enabled"] as? Bool, true)
        XCTAssertNil(installationObject["app_topic"])
    }

    func testTypedDeepLinkRejectsArbitraryWebURL() throws {
        let url = try XCTUnwrap(URL(string: "https://example.com/briefing/2026-08-02/morning"))
        XCTAssertNil(AppRoute(url: url))
    }

    func testBriefingPathMatchesDeployedRoute() {
        XCTAssertEqual(
            BrunnAPI.briefingPath(date: "2026-08-02", edition: "morning"),
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
              "first_headline": "**One** useful [line](https://example.com).",
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
        XCTAssertEqual(envelope.data.editions.first?.firstHeadline, "**One** useful [line](https://example.com).")
        XCTAssertEqual(envelope.data.editions.first?.sectionTitles, ["Projects", "News"])
        XCTAssertEqual(envelope.data.editions.first?.itemCount, 4)

        let empty = #"""
        {"date":"2026-08-01","edition":"evening","path":"Briefings/2026/Evening briefing - 2026-08-01.md","entry_ref":"entry:evening","version":1,"generated_at":null,"first_headline":null,"section_titles":[],"item_count":0}
        """#.data(using: .utf8)!
        XCTAssertNil(try JSONDecoder().decode(BriefingListRow.self, from: empty).firstHeadline)
    }

    func testTopicsSnapshotDecodesTruncationAndPendingRequestSignals() throws {
        let json = #"""
        {
          "status": "complete",
          "data": {
            "topics": [{
              "slug": "brunn",
              "name": "Brunn",
              "section_order": 10,
              "mode": "every_briefing",
              "editions": ["morning", "evening"],
              "schedule": "06:30",
              "entities": ["Brunn"],
              "symbols": ["STRAY"],
              "suppress_unchanged": true,
              "freshness_hours": 12,
              "body": "Product and reliability updates.",
              "truncated": true,
              "parse_error": "The trailing instructions were truncated.",
              "path": "Briefings/Topics/brunn.md",
              "entry_ref": "entry:topic-brunn",
              "version": 4
            }],
            "pending_requests": [{
              "path": "Briefings/Requests/2026-08-02 - native-ios.md",
              "entry_ref": "entry:request-native-ios",
              "date": "2026-08-02",
              "item_id": "native-ios",
              "edition_ref": "entry:morning",
              "topic": "brunn",
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
            topicSlug: "brunn",
            verdict: "follow_closer",
            note: "Go deeper."
        )

        let data = try JSONEncoder().encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(object["action"] as? String, "expand")
        XCTAssertEqual(object["edition_ref"] as? String, "entry:morning")
        XCTAssertEqual(object["item_id"] as? String, "native-ios")
        XCTAssertEqual(object["topic_slug"] as? String, "brunn")
        XCTAssertEqual(object["verdict"] as? String, "follow_closer")
        XCTAssertEqual(object["note"] as? String, "Go deeper.")
        XCTAssertNil(object["editionRef"])
        XCTAssertNil(object["itemID"])
        XCTAssertNil(object["topicSlug"])
    }

    func testTaskRouteRequiresCanonicalLowercaseUUIDv7() throws {
        let valid = "019f8800-0000-7000-8000-000000000001"
        XCTAssertEqual(
            AppRoute(url: try XCTUnwrap(URL(string: "brunn://task/\(valid)"))),
            .task(reference: valid)
        )
        XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(string: "brunn://task/\(valid.uppercased())"))))
        XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(string: "brunn://task/019f8800-0000-4000-8000-000000000001"))))
        XCTAssertNil(AppRoute(url: try XCTUnwrap(URL(string: "brunn://task/task:\(valid)"))))
    }

    func testTodayProjectionIsUniqueBoundedAndUsesContextANDSemantics() {
        func candidate(
            _ suffix: Int,
            contexts: [String] = [],
            pinned: Bool = false
        ) -> AgentTaskCandidate {
            AgentTaskCandidate(
                taskRef: String(format: "019f8800-0000-7000-8000-%012d", suffix),
                entryRef: "entry:\(suffix)",
                version: 1,
                title: "Task \(suffix)",
                requiredContexts: contexts,
                tier: suffix <= 2 ? 1 : 5,
                reason: "deterministic",
                pinned: pinned
            )
        }
        let urgent = [candidate(1), candidate(1), candidate(2)]
        let next = urgent + [
            candidate(3, pinned: true),
            candidate(4, pinned: true),
            candidate(5),
            candidate(6),
            candidate(7),
            candidate(8),
            candidate(9, contexts: ["home", "online"]),
        ]

        let projection = AgentTaskTodayProjection.bounded(
            urgent: urgent,
            next: next,
            contextsAvailable: ["phone", "online"]
        )

        XCTAssertLessThanOrEqual(projection.all.count, 7)
        XCTAssertEqual(Set(projection.all.map(\.taskRef)).count, projection.all.count)
        XCTAssertFalse(projection.all.contains { $0.title == "Task 9" })
        XCTAssertEqual(projection.all.filter(\.pinned).count, 2)

        let emptyUrgent = AgentTaskTodayProjection.bounded(
            urgent: [],
            next: next,
            contextsAvailable: ["phone", "online"]
        )
        XCTAssertTrue(emptyUrgent.urgent.isEmpty)
        XCTAssertFalse(emptyUrgent.next.isEmpty)
    }

    func testProjectProjectionShowsTenUniqueNextTasksBeforeWaiting() {
        func candidate(_ suffix: Int) -> AgentTaskCandidate {
            AgentTaskCandidate(
                taskRef: String(format: "019f8800-0000-7000-8000-%012d", suffix),
                entryRef: "entry:\(suffix)",
                version: 1,
                title: "Task \(suffix)",
                tier: 5,
                reason: "project order"
            )
        }
        func waiting(_ suffix: Int) -> AgentTaskWaitingItem {
            AgentTaskWaitingItem(
                taskRef: String(format: "019f8800-0000-7000-8000-%012d", suffix),
                title: "Waiting \(suffix)",
                since: "2026-08-20T12:00:00Z",
                ageDays: 7
            )
        }

        let projection = AgentTaskProjectProjection.bounded(
            next: (1...12).map(candidate),
            waiting: [waiting(3), waiting(13), waiting(14), waiting(15), waiting(16), waiting(17), waiting(18)]
        )

        XCTAssertEqual(projection.next.count, 10)
        XCTAssertEqual(projection.waiting.count, 5)
        XCTAssertEqual(projection.taskCount, 15)
        XCTAssertEqual(
            Set((projection.next.map(\.taskRef) + projection.waiting.map(\.taskRef))).count,
            projection.taskCount
        )
    }

    func testTaskCandidateAndTypedNotificationTargetDecodeExactWireShape() throws {
        let json = #"""
        {
          "status":"complete",
          "data":{
            "view":"next",
            "as_of":"2026-08-27T12:00:00Z",
            "contexts_available":["phone","online"],
            "items":[{
              "task_ref":"019f8800-0000-7000-8000-000000000001",
              "entry_ref":"entry:one",
              "version":3,
              "title":"Call the pharmacy",
              "status":"open",
              "project":"health",
              "required_contexts":["phone"],
              "tier":3,
              "reason":"should do by Fri (est.)",
              "provenance_markers":["agent:aether"],
              "pinned":false,
              "estimate_minutes":10,
              "today_since":"2026-08-25"
            }],
            "urgent_total":0,
            "next_remaining":8,
            "backlog_total":19,
            "next_cursor":null
          }
        }
        """#.data(using: .utf8)!
        let envelope = try JSONDecoder().decode(
            WorkspaceEnvelope<AgentTaskCandidatesData>.self,
            from: json
        )
        XCTAssertEqual(envelope.data.items.first?.project, "health")
        XCTAssertEqual(envelope.data.items.first?.hasInferredProvenance, true)
        XCTAssertEqual(envelope.data.items.first?.estimateMinutes, 10)
        XCTAssertEqual(envelope.data.items.first?.todaySince, "2026-08-25")
        XCTAssertEqual(envelope.data.nextRemaining, 8)

        let targetJSON = #"""
        {"type":"task","task_ref":"019f8800-0000-7000-8000-000000000001"}
        """#.data(using: .utf8)!
        let target = try JSONDecoder().decode(BrunnNotificationTarget.self, from: targetJSON)
        XCTAssertEqual(target.type, .task)
        XCTAssertEqual(target.taskRef, "019f8800-0000-7000-8000-000000000001")
    }

    func testTaskUpdateEncodesCASOwnerSourceAndIOSCompletion() throws {
        let request = AgentTaskUpdateRequest(
            expectedVersion: 7,
            idempotencyKey: "ios:test:complete",
            operation: .complete
        )
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any]
        )
        let operation = try XCTUnwrap(object["operation"] as? [String: Any])
        XCTAssertEqual(object["expected_version"] as? Int, 7)
        XCTAssertEqual(object["idempotency_key"] as? String, "ios:test:complete")
        XCTAssertEqual(operation["type"] as? String, "complete")
        XCTAssertEqual(operation["source"] as? String, "owner")
        XCTAssertEqual(operation["completed_via"] as? String, "ios")
    }

    func testTaskDeletionEncodesRecoverableDropWithoutCompletion() throws {
        let request = AgentTaskUpdateRequest(expectedVersion: 7, idempotencyKey: "ios:test:delete", operation: .drop)
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any]
        )
        XCTAssertEqual(object["expected_version"] as? Int, 7)
        XCTAssertEqual(object["idempotency_key"] as? String, "ios:test:delete")
        let operation = try XCTUnwrap(object["operation"] as? [String: String])
        XCTAssertEqual(operation, ["type": "drop", "source": "owner", "reason": "Deleted from iOS"])
    }

    func testTaskCandidateDecodesWithoutEstimateOrTodayFields() throws {
        let json = #"""
        {"task_ref":"019f8800-0000-7000-8000-000000000002","entry_ref":"entry:two","version":1,"title":"Older server","status":"open","project":null,"required_contexts":[],"tier":5,"reason":"ready","provenance_markers":[],"pinned":false}
        """#.data(using: .utf8)!
        let candidate = try JSONDecoder().decode(AgentTaskCandidate.self, from: json)
        XCTAssertNil(candidate.estimateMinutes)
        XCTAssertNil(candidate.todaySince)
    }

    func testTaskUpdateEncodesTodayListActions() throws {
        func type(of operation: AgentTaskUpdateOperation) throws -> String? {
            let request = AgentTaskUpdateRequest(expectedVersion: 1, idempotencyKey: "ios:test", operation: operation)
            let object = try XCTUnwrap(
                JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any]
            )
            return (object["operation"] as? [String: Any])?["type"] as? String
        }
        XCTAssertEqual(try type(of: .addToday), "add_today")
        XCTAssertEqual(try type(of: .sweep), "sweep")
    }

    func testTaskCaptureEncodesTodayFlagAndIOSOrigin() throws {
        let request = AgentTaskCaptureRequest(rawText: "Call the vet", idempotencyKey: "ios_task_capture_test")
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any]
        )
        let item = try XCTUnwrap((object["items"] as? [[String: Any]])?.first)
        XCTAssertEqual(object["idempotency_key"] as? String, "ios_task_capture_test")
        XCTAssertEqual(item["raw_text"] as? String, "Call the vet")
        XCTAssertEqual(item["captured_from"] as? String, "ios:today")
        XCTAssertEqual(item["today"] as? Bool, true)
        XCTAssertTrue(AgentTaskCaptureRequest(rawText: "x").idempotencyKey.hasPrefix("ios_task_capture_"))

        let response = #"""
        {"status":"committed","data":{"items":[{"client_ref":null,"task_ref":"019f8800-0000-7000-8000-000000000009","entry_ref":"entry:019f8800-0000-7000-8000-000000000009","version":1,"title":"Call the vet","enrichment":{"title":"Call the vet"},"context_suggestions":[],"suggested_existing":[]}],"replayed":false}}
        """#.data(using: .utf8)!
        let envelope = try JSONDecoder().decode(WorkspaceEnvelope<AgentTaskCaptureData>.self, from: response)
        XCTAssertEqual(envelope.data.items.count, 1)
        XCTAssertEqual(envelope.data.items.first?.taskRef, "019f8800-0000-7000-8000-000000000009")
        XCTAssertEqual(envelope.data.items.first?.title, "Call the vet")
        XCTAssertEqual(envelope.data.replayed, false)
    }

    func testProjectStatusDecodesTolerantlyFromOlderAndNewerServers() throws {
        let older = #"""
        {"projects":[{"slug":"brunn","title":"Brunn","interest":"hot","last_activity_at":null,"open_task_count":2,"last_checkpoint_at":null,"version":1}],"as_of":"2026-09-11T06:00:00Z"}
        """#.data(using: .utf8)!
        let legacy = try JSONDecoder().decode(AgentTaskProjectListData.self, from: older)
        XCTAssertNil(legacy.projects.first?.status)

        let newer = #"""
        {"projects":[{"slug":"brunn","title":"Brunn","interest":"hot","last_activity_at":null,"open_task_count":2,"last_checkpoint_at":null,"version":1,"status":"red","status_reason":"Deadline slipped.","status_since":"2026-09-10","status_previous":"yellow","status_computed_on":"2026-09-11"},{"slug":"odd","title":"Odd","interest":"normal","open_task_count":0,"version":1,"status":"purple","status_reason":"","status_since":null,"status_previous":null,"status_computed_on":null}],"as_of":"2026-09-11T06:00:00Z"}
        """#.data(using: .utf8)!
        let current = try JSONDecoder().decode(AgentTaskProjectListData.self, from: newer)
        XCTAssertEqual(current.projects[0].status, .red)
        XCTAssertEqual(current.projects[0].statusReason, "Deadline slipped.")
        XCTAssertEqual(current.projects[0].statusSince, "2026-09-10")
        XCTAssertEqual(current.projects[0].statusPrevious, "yellow")
        XCTAssertEqual(current.projects[0].statusComputedOn, "2026-09-11")
        XCTAssertEqual(current.projects[1].status, .grey, "Unknown colours must fall back to grey")

        let state = #"""
        {"project":{"slug":"brunn","title":"Brunn","interest":"hot","last_activity_at":null,"version":1,"status":"green","status_reason":"On track.","status_since":"2026-09-01","status_previous":null,"status_computed_on":"2026-09-11"},"checkpoint":null,"urgent_count":0,"next":[],"waiting":[],"waiting_total":0,"waiting_remaining":0,"parked_count":0,"as_of":"2026-09-11T06:00:00Z"}
        """#.data(using: .utf8)!
        let decoded = try JSONDecoder().decode(AgentTaskProjectStateData.self, from: state)
        XCTAssertEqual(decoded.project.status, .green)
        XCTAssertEqual(decoded.project.statusReason, "On track.")
    }

    func testOwnerDayCountsWholeLocalDays() throws {
        let timezone = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timezone
        // 2026-09-11 at 00:30 local: the previous UTC day has not ended locally.
        let now = try XCTUnwrap(calendar.date(from: DateComponents(year: 2026, month: 9, day: 11, hour: 0, minute: 30)))
        XCTAssertEqual(OwnerDay.daysSince("2026-09-11", now: now, timezone: timezone), 0)
        XCTAssertEqual(OwnerDay.daysSince("2026-09-10", now: now, timezone: timezone), 1)
        XCTAssertEqual(OwnerDay.daysSince("2026-09-01", now: now, timezone: timezone), 10)
        XCTAssertNil(OwnerDay.daysSince("yesterday", now: now, timezone: timezone))
        XCTAssertNil(OwnerDay.daysSince(nil, now: now, timezone: timezone))
    }
}
