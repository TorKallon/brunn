import Foundation

enum SampleData {
    static let briefing = BriefingEditionData(
        path: "Briefings/2026/Morning briefing - 2026-08-02.md",
        entryRef: "entry:demo-morning",
        version: 2,
        currentVersion: 2,
        date: "2026-08-02",
        edition: "morning",
        briefing: BriefingPayload(
            date: "2026-08-02",
            edition: "morning",
            timezone: "America/Los_Angeles",
            generatedAt: "2026-08-02T06:30:00-07:00",
            summaryMD: [
                "The **native iOS reader** now follows the dense, full-width mobile briefing layout.",
                "Today exposes the complete summary, every section, and expandable source-backed details.",
                "News collects the latest new, updated, and corrected items without an assistant hop.",
                "Archive preserves date, edition, and revision navigation through deployed briefing APIs.",
                "Tracked topics and pending deep-dives are visible without turning the phone into a second notes store.",
                "The latest edition remains available in a bounded, data-protected offline cache.",
                "Private briefing prose stays out of default lock-screen payloads until an authenticated APNs delivery service exists.",
            ],
            sections: [
                BriefingSection(
                    topic: "straylight",
                    title: "Straylight",
                    items: [
                        BriefingItem(
                            id: "ios-direction",
                            kind: "project",
                            headlineMD: "**Native iOS becomes Straylight's direct mobile surface.**",
                            bodyMD: "The app centers the morning thread, durable-context lookup, task state, and alert delivery without copying the whole workspace.",
                            whyItMatters: "It removes the assistant hop for common mobile interactions while keeping hosted Straylight authoritative.",
                            detailMD: "The first end-to-end slice is connection, latest briefing, protected cache, notification routing, and exact item navigation.",
                            whatChanged: "The earlier PWA-first sequencing decision is superseded for this owner alpha.",
                            delta: "update",
                            story: BriefingStory(
                                key: "straylight-native-ios",
                                urls: ["https://developer.apple.com/documentation/swiftui"],
                                title: "Straylight native iOS direction",
                                entities: ["Straylight", "SwiftUI"]
                            ),
                            times: BriefingTimes(
                                publishedAt: "2026-08-02T06:30:00-07:00",
                                firstSeenAt: "2026-08-02T06:20:00-07:00"
                            )
                        ),
                    ]
                ),
                BriefingSection(
                    topic: "platform",
                    title: "Platform",
                    items: [
                        BriefingItem(
                            id: "existing-contracts",
                            kind: "ops",
                            headlineMD: "**Briefing and search APIs can power the first useful build today.**",
                            bodyMD: "Structured `briefing.v1` list/detail and workspace search/read are deployed behind bearer authentication.",
                            whyItMatters: "The scaffold can exercise real data while task and push work stays safely feature-gated.",
                            delta: "new",
                            story: BriefingStory(
                                key: "straylight-ios-existing-contracts",
                                urls: ["https://straylight.rourkem.com"],
                                title: "Deployed Straylight briefing contracts"
                            ),
                            times: BriefingTimes(publishedAt: "2026-08-02T06:32:00-07:00")
                        ),
                        BriefingItem(
                            id: "delivery-correction",
                            kind: "correction",
                            headlineMD: "**A delivery status label was corrected to distinguish briefing inclusion from push delivery.**",
                            bodyMD: "The briefing story ledger records inclusion in an edition; it does not prove that APNs, email, or iMessage delivered anything.",
                            whyItMatters: "The app should never imply a notification was delivered when the server has no device or receipt ledger.",
                            detailMD: "News therefore presents source-backed briefing activity and uses local session read state only.",
                            whatChanged: "The earlier copy called briefing-derived updates delivered alerts.",
                            delta: "update",
                            story: BriefingStory(
                                key: "straylight-delivery-language",
                                urls: ["https://developer.apple.com/documentation/usernotifications"],
                                title: "Notification delivery semantics"
                            ),
                            times: BriefingTimes(
                                publishedAt: "2026-08-02T10:00:00-07:00",
                                firstSeenAt: "2026-08-02T09:55:00-07:00"
                            )
                        ),
                    ]
                ),
                BriefingSection(
                    topic: "reading-experience",
                    title: "Reading experience",
                    items: [
                        BriefingItem(
                            id: "full-width-reader",
                            kind: "design",
                            headlineMD: "**The decorative timeline rail was removed so agent prose gets the screen.**",
                            bodyMD: "Phone gutters are compact, summary bullets use the full readable width, and section rows expand in place.",
                            whyItMatters: "Dense morning briefings stay scannable without shrinking type or hiding source context.",
                            delta: "new",
                            story: BriefingStory(
                                key: "straylight-full-width-reader",
                                urls: ["https://developer.apple.com/design/human-interface-guidelines/layout"],
                                title: "Full-width briefing reader"
                            ),
                            times: BriefingTimes(firstSeenAt: "2026-08-02T10:05:00-07:00")
                        ),
                    ]
                ),
            ],
            delta: BriefingDelta(
                added: ["existing-contracts", "full-width-reader"],
                changed: ["ios-direction", "delivery-correction"]
            )
        ),
        markdown: "# Morning briefing - 2026-08-02\n\nDemo content.",
        createdAt: "2026-08-02T06:30:00-07:00",
        versions: [
            BriefingEditionVersion(version: 1, createdAt: "2026-08-02T06:30:00-07:00"),
            BriefingEditionVersion(version: 2, createdAt: "2026-08-02T10:00:00-07:00"),
        ],
        workspaceGeneration: 1
    )

    static let briefingHistory: [BriefingListRow] = [
        BriefingListRow(
            date: "2026-08-02",
            edition: "morning",
            path: briefing.path,
            entryRef: briefing.entryRef,
            version: briefing.currentVersion,
            generatedAt: briefing.briefing?.generatedAt,
            summaryMD: briefing.briefing?.summaryMD ?? [],
            sectionTitles: briefing.briefing?.sections?.map(\.title) ?? [],
            itemCount: briefing.briefing?.sections?.reduce(0) { $0 + $1.items.count } ?? 0
        ),
        BriefingListRow(
            date: "2026-08-01",
            edition: "evening",
            path: "Briefings/2026/Evening briefing - 2026-08-01.md",
            entryRef: "entry:demo-evening",
            version: 1,
            generatedAt: "2026-08-01T17:30:00-07:00",
            summaryMD: ["A compact end-of-day edition with two source-backed updates."],
            sectionTitles: ["Straylight", "Reading experience"],
            itemCount: 2
        ),
        BriefingListRow(
            date: "2026-08-01",
            edition: "morning",
            path: "Briefings/2026/Morning briefing - 2026-08-01.md",
            entryRef: "entry:demo-prior-morning",
            version: 3,
            generatedAt: "2026-08-01T06:30:00-07:00",
            summaryMD: ["The prior morning edition remains available by date and revision."],
            sectionTitles: ["Platform"],
            itemCount: 1
        ),
    ]

    static let dashboard = WorkspaceDashboardData(
        generatedAt: "2026-08-02T16:30:00-07:00",
        timezone: "America/Los_Angeles",
        workspaceGeneration: 24_136,
        activityTrackingStartedAt: "2026-07-27T00:00:00Z",
        storage: DashboardStorage(
            text: DashboardStorageMetric(count: 4_926, sizeBytes: 298_682_825),
            binary: DashboardStorageMetric(
                count: 73,
                sizeBytes: 797_775_263,
                semantics: "current_referenced_objects"
            )
        ),
        today: DashboardTodayActivity(
            readOperations: 146,
            readBytes: 8_724_480,
            writeOperations: 18,
            writeBytes: 486_400
        ),
        activity: [
            DashboardActivityPoint(date: "2026-07-27", readOperations: 82, readBytes: 4_820_000, writeOperations: 12, writeBytes: 310_000),
            DashboardActivityPoint(date: "2026-07-28", readOperations: 104, readBytes: 6_140_000, writeOperations: 9, writeBytes: 205_000),
            DashboardActivityPoint(date: "2026-07-29", readOperations: 71, readBytes: 3_900_000, writeOperations: 16, writeBytes: 390_000),
            DashboardActivityPoint(date: "2026-07-30", readOperations: 128, readBytes: 7_480_000, writeOperations: 14, writeBytes: 418_000),
            DashboardActivityPoint(date: "2026-07-31", readOperations: 116, readBytes: 6_920_000, writeOperations: 21, writeBytes: 602_000),
            DashboardActivityPoint(date: "2026-08-01", readOperations: 93, readBytes: 5_440_000, writeOperations: 11, writeBytes: 274_000),
            DashboardActivityPoint(date: "2026-08-02", readOperations: 146, readBytes: 8_724_480, writeOperations: 18, writeBytes: 486_400),
        ],
        access: [
            DashboardAccessClient(
                id: "credential:demo-iphone",
                name: "Rourke’s iPhone",
                access: "read_only",
                status: "active",
                scopeIDs: ["scope:root"],
                capabilities: ["open", "query", "read", "compute", "verify", "status"],
                createdAt: "2026-08-02T12:00:00-07:00",
                lastUsedAt: "2026-08-02T16:28:00-07:00",
                lastOperation: "read",
                readOperationsToday: 14
            ),
            DashboardAccessClient(
                id: "credential:demo-codex",
                name: "Codex on Nyx",
                access: "read_write",
                status: "active",
                scopeIDs: ["scope:root"],
                lastUsedAt: "2026-08-02T16:29:00-07:00",
                lastOperation: "write",
                readOperationsToday: 96,
                writeOperationsToday: 18
            ),
            DashboardAccessClient(
                id: "credential:demo-retired",
                name: "Retired test token",
                access: "read_only",
                status: "revoked",
                scopeIDs: ["scope:root"],
                revokedAt: "2026-08-01T20:00:00-07:00",
                lastUsedAt: "2026-08-01T19:42:00-07:00"
            ),
        ],
        coverage: DashboardCoverage(days: 7, activity: "tracked_operations_only")
    )

    static let topicsSnapshot = BriefingTopicsSnapshot(
        topics: [
            BriefingTopic(
                slug: "straylight",
                name: "Straylight",
                sectionOrder: 10,
                mode: "every_briefing",
                editions: ["morning", "evening"],
                entities: ["Straylight"],
                suppressUnchanged: true,
                freshnessHours: 12,
                body: "Product, reliability, and durable-context changes that affect daily work.",
                path: "Briefings/Topics/straylight.md",
                entryRef: "entry:topic-straylight",
                version: 4
            ),
            BriefingTopic(
                slug: "reading-experience",
                name: "Reading experience",
                sectionOrder: 20,
                mode: "on_material_delta",
                editions: ["morning"],
                entities: ["SwiftUI"],
                suppressUnchanged: true,
                freshnessHours: 24,
                body: "Only material changes to how agent-produced content is read and acted on.",
                path: "Briefings/Topics/reading-experience.md",
                entryRef: "entry:topic-reading",
                version: 2
            ),
        ],
        pendingRequests: [
            BriefingPendingRequest(
                path: "Briefings/Requests/2026-08-02 - full-width-reader.md",
                entryRef: "entry:request-reader",
                date: "2026-08-02",
                itemID: "full-width-reader",
                editionRef: briefing.entryRef,
                topic: "reading-experience",
                note: "Compare reading density at the largest accessibility text size."
            ),
        ],
        feedbackPath: "Briefings/Feedback/2026-08.md",
        feedbackTail: [],
        workspaceGeneration: 1
    )

    static let initiallyReadNewsItemIDs: Set<String> = ["existing-contracts"]

    static func briefing(date: String, edition: String, version: Int?) -> BriefingEditionData {
        guard date != briefing.date || edition != briefing.edition else {
            if let version, version == 1 {
                return copyBriefing(date: date, edition: edition, version: 1, currentVersion: 2)
            }
            return briefing
        }
        let row = briefingHistory.first { $0.date == date && $0.edition == edition }
        return copyBriefing(
            date: date,
            edition: edition,
            version: version ?? row?.version ?? 1,
            currentVersion: row?.version ?? 1
        )
    }

    private static func copyBriefing(
        date: String,
        edition: String,
        version: Int,
        currentVersion: Int
    ) -> BriefingEditionData {
        BriefingEditionData(
            path: "Briefings/2026/\(edition.capitalized) briefing - \(date).md",
            entryRef: "entry:demo-\(date)-\(edition)",
            version: version,
            currentVersion: currentVersion,
            date: date,
            edition: edition,
            briefing: BriefingPayload(
                date: date,
                edition: edition,
                timezone: briefing.briefing?.timezone,
                generatedAt: "\(date)T06:30:00-07:00",
                summaryMD: ["A source-backed \(edition) edition from \(date)."],
                sections: Array((briefing.briefing?.sections ?? []).prefix(1))
            ),
            markdown: "# \(edition.capitalized) briefing - \(date)",
            createdAt: "\(date)T06:30:00-07:00",
            versions: (1 ... max(currentVersion, 1)).map {
                BriefingEditionVersion(version: $0, createdAt: "\(date)T0\(min(6 + $0, 9)):30:00-07:00")
            },
            workspaceGeneration: 1
        )
    }

    static let tasks: [TaskItem] = [
        TaskItem(
            id: "demo-task-1",
            title: "Define the APNs device-registration contract",
            note: "Token upsert, revoke, preferences, outbox, attempts, open receipt.",
            state: .open,
            context: "computer",
            estimatedMinutes: 30,
            reason: "Unlocks replacement of the iMessage delivery path."
        ),
        TaskItem(
            id: "demo-task-2",
            title: "Add a Markdown-derived task projection",
            note: "Preserve canonical task notes and revision-guard every mutation.",
            state: .open,
            context: "computer",
            estimatedMinutes: 45,
            reason: "Needed before the native task list can become authoritative."
        ),
        TaskItem(
            id: "demo-task-3",
            title: "Run the first signed-device push canary",
            note: "Keep iMessage dual delivery until open receipts prove the new path.",
            state: .waiting,
            context: "phone",
            estimatedMinutes: 15,
            reason: "Depends on signing and the server registration endpoint."
        ),
    ]

    static let alerts: [AlertItem] = [
        AlertItem(
            id: "demo-alert-1",
            topic: "STRAYLIGHT · MATERIAL UPDATE",
            headline: "Native iOS is now the active mobile direction.",
            detail: "The scaffold starts with briefing and search contracts that already exist.",
            kind: .update,
            deliveredAt: Date(timeIntervalSince1970: 1_775_332_800),
            whatChanged: "The previous PWA-first sequencing decision is superseded."
        ),
        AlertItem(
            id: "demo-alert-2",
            topic: "PUSH · WATCHING",
            headline: "APNs delivery is not connected to Straylight yet.",
            detail: "Permission and typed routing exist locally; device registration and delivery receipts remain server work.",
            kind: .watching,
            deliveredAt: Date(timeIntervalSince1970: 1_775_336_400)
        ),
    ]

    static let searchResults: [WorkspaceSearchCandidate] = [
        WorkspaceSearchCandidate(
            reference: "entry:demo-ios-mvp",
            path: "docs/ios/MVP.md",
            title: "Straylight iOS MVP",
            version: 1,
            heading: "First vertical slice",
            excerpt: "Connection, latest structured briefing, protected cache, typed notification route, and exact source navigation.",
            lanes: ["exact"]
        ),
        WorkspaceSearchCandidate(
            reference: "entry:demo-briefing-design",
            path: "docs/superpowers/specs/2026-08-01-briefings-design.md",
            title: "Straylight Briefings: Platform Design",
            version: 1,
            heading: "Architecture",
            excerpt: "Agents research and generate; Straylight stores Markdown editions, dedupe state, topics, and the Daily Thread projection.",
            lanes: ["lexical"]
        ),
    ]
}
