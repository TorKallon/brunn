import Foundation

public typealias CycleMilliseconds = Int64

public enum SourcePhase: String, Codable, CaseIterable, Sendable {
    case burning
    case cooling
    case stabilizing
    case stable
}

public enum MissionPhase: String, Codable, CaseIterable, Sendable {
    case fireWave
    case cooling
    case recovery
    case worldActive
    case rupturaWarning
}

public enum TimelineEventCategory: String, Codable, Sendable {
    case phase
    case enemy
    case ruptura
}

public enum AlertSeverity: Int, Codable, Comparable, Sendable {
    case informational = 0
    case notice = 1
    case warning = 2
    case urgent = 3
    case critical = 4

    public static func < (lhs: AlertSeverity, rhs: AlertSeverity) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public enum EvidenceClass: String, Codable, Sendable {
    case official
    case sourceData
    case communityDerived
    case experimental
}

public enum OpportunityConfidence: String, Codable, Sendable {
    case high
    case medium
    case low
    case experimental
}

public struct RuptureCycleVersion: Codable, Equatable, Sendable {
    public let definitionID: String
    public let sourceSnapshotID: String
    public let sourceClaimedGameBuildID: String
    public let latestPublicBuildAtCaptureID: String
    public let sourceVersion: String
    public let capturedAtISO8601: String

    public init(
        definitionID: String,
        sourceSnapshotID: String,
        sourceClaimedGameBuildID: String,
        latestPublicBuildAtCaptureID: String,
        sourceVersion: String,
        capturedAtISO8601: String
    ) {
        self.definitionID = definitionID
        self.sourceSnapshotID = sourceSnapshotID
        self.sourceClaimedGameBuildID = sourceClaimedGameBuildID
        self.latestPublicBuildAtCaptureID = latestPublicBuildAtCaptureID
        self.sourceVersion = sourceVersion
        self.capturedAtISO8601 = capturedAtISO8601
    }
}

public struct CycleRange: Codable, Equatable, Sendable {
    public let startMilliseconds: CycleMilliseconds
    public let endMilliseconds: CycleMilliseconds

    public init(
        startMilliseconds: CycleMilliseconds,
        endMilliseconds: CycleMilliseconds
    ) {
        self.startMilliseconds = startMilliseconds
        self.endMilliseconds = endMilliseconds
    }

    public var durationMilliseconds: CycleMilliseconds {
        endMilliseconds - startMilliseconds
    }

    public func contains(_ offsetMilliseconds: CycleMilliseconds) -> Bool {
        startMilliseconds <= offsetMilliseconds && offsetMilliseconds < endMilliseconds
    }
}

public struct SourcePhaseDefinition: Codable, Equatable, Sendable {
    public let phase: SourcePhase
    public let range: CycleRange

    public init(phase: SourcePhase, range: CycleRange) {
        self.phase = phase
        self.range = range
    }
}

public struct MissionBandDefinition: Codable, Equatable, Sendable {
    public let phase: MissionPhase
    public let range: CycleRange
    public let title: String
    public let threatInstruction: String
    public let heroEventID: String

    public init(
        phase: MissionPhase,
        range: CycleRange,
        title: String,
        threatInstruction: String,
        heroEventID: String
    ) {
        self.phase = phase
        self.range = range
        self.title = title
        self.threatInstruction = threatInstruction
        self.heroEventID = heroEventID
    }
}

public struct TimelineEventDefinition: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let offsetMilliseconds: CycleMilliseconds
    public let category: TimelineEventCategory
    public let title: String
    public let target: String
    public let severity: AlertSeverity
    public let isAlert: Bool

    public init(
        id: String,
        offsetMilliseconds: CycleMilliseconds,
        category: TimelineEventCategory,
        title: String,
        target: String,
        severity: AlertSeverity,
        isAlert: Bool
    ) {
        self.id = id
        self.offsetMilliseconds = offsetMilliseconds
        self.category = category
        self.title = title
        self.target = target
        self.severity = severity
        self.isAlert = isAlert
    }
}

public struct OpportunityDefinition: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let range: CycleRange
    public let title: String
    public let action: String
    public let caveat: String
    public let evidenceClass: EvidenceClass
    public let confidence: OpportunityConfidence
    public let defaultRank: Int

    public init(
        id: String,
        range: CycleRange,
        title: String,
        action: String,
        caveat: String,
        evidenceClass: EvidenceClass,
        confidence: OpportunityConfidence,
        defaultRank: Int
    ) {
        self.id = id
        self.range = range
        self.title = title
        self.action = action
        self.caveat = caveat
        self.evidenceClass = evidenceClass
        self.confidence = confidence
        self.defaultRank = defaultRank
    }
}

public enum CycleDefinitionValidationError: Error, Equatable, Sendable {
    case invalidCycleLength
    case emptySourcePhases
    case emptyMissionBands
    case invalidCoverage(String)
    case duplicateID(String)
    case invalidEventOffset(String)
    case invalidOpportunityRange(String)
    case missingHeroEvent(String)
}

public struct RuptureCycleDefinition: Codable, Equatable, Sendable {
    public let version: RuptureCycleVersion
    public let cycleLengthMilliseconds: CycleMilliseconds
    public let sourcePhases: [SourcePhaseDefinition]
    public let missionBands: [MissionBandDefinition]
    public let events: [TimelineEventDefinition]
    public let opportunities: [OpportunityDefinition]

    public init(
        version: RuptureCycleVersion,
        cycleLengthMilliseconds: CycleMilliseconds,
        sourcePhases: [SourcePhaseDefinition],
        missionBands: [MissionBandDefinition],
        events: [TimelineEventDefinition],
        opportunities: [OpportunityDefinition]
    ) {
        self.version = version
        self.cycleLengthMilliseconds = cycleLengthMilliseconds
        self.sourcePhases = sourcePhases
        self.missionBands = missionBands
        self.events = events
        self.opportunities = opportunities
    }

    public func validate() throws {
        guard cycleLengthMilliseconds > 0 else {
            throw CycleDefinitionValidationError.invalidCycleLength
        }
        guard !sourcePhases.isEmpty else {
            throw CycleDefinitionValidationError.emptySourcePhases
        }
        guard !missionBands.isEmpty else {
            throw CycleDefinitionValidationError.emptyMissionBands
        }

        try validateCoverage(
            sourcePhases.map { ($0.phase.rawValue, $0.range) },
            layer: "source"
        )
        try validateCoverage(
            missionBands.map { ($0.phase.rawValue, $0.range) },
            layer: "mission"
        )

        try validateUnique(sourcePhases.map { $0.phase.rawValue })
        try validateUnique(missionBands.map { $0.phase.rawValue })
        try validateUnique(events.map(\.id))
        try validateUnique(opportunities.map(\.id))

        for event in events where !(0..<cycleLengthMilliseconds).contains(event.offsetMilliseconds) {
            throw CycleDefinitionValidationError.invalidEventOffset(event.id)
        }

        for opportunity in opportunities {
            let range = opportunity.range
            guard range.startMilliseconds >= 0,
                  range.startMilliseconds < range.endMilliseconds,
                  range.endMilliseconds <= cycleLengthMilliseconds
            else {
                throw CycleDefinitionValidationError.invalidOpportunityRange(opportunity.id)
            }
        }

        let eventIDs = Set(events.map(\.id))
        for band in missionBands where !eventIDs.contains(band.heroEventID) {
            throw CycleDefinitionValidationError.missingHeroEvent(band.heroEventID)
        }
    }

    private func validateCoverage(
        _ segments: [(String, CycleRange)],
        layer: String
    ) throws {
        var expectedStart: CycleMilliseconds = 0
        for (id, range) in segments {
            guard range.startMilliseconds == expectedStart,
                  range.endMilliseconds > range.startMilliseconds,
                  range.endMilliseconds <= cycleLengthMilliseconds
            else {
                throw CycleDefinitionValidationError.invalidCoverage("\(layer).\(id)")
            }
            expectedStart = range.endMilliseconds
        }
        guard expectedStart == cycleLengthMilliseconds else {
            throw CycleDefinitionValidationError.invalidCoverage(layer)
        }
    }

    private func validateUnique(_ ids: [String]) throws {
        var seen = Set<String>()
        for id in ids where !seen.insert(id).inserted {
            throw CycleDefinitionValidationError.duplicateID(id)
        }
    }
}

extension RuptureCycleDefinition {
    static let currentR1 = RuptureCycleDefinition(
        version: RuptureCycleVersion(
            definitionID: "ruptura-cycle/u1-srdb-2.3.4/r1",
            sourceSnapshotID: "ea-u1-gv35118973__srdb-2.3.4__20260711T050230Z",
            sourceClaimedGameBuildID: "ea_u1_0.2.0_build_22674441",
            latestPublicBuildAtCaptureID: "ea_u1_0.2.8_build_23761620",
            sourceVersion: "SRDB 2.3.4",
            capturedAtISO8601: "2026-07-11T05:02:30.009644Z"
        ),
        cycleLengthMilliseconds: 3_240_000,
        sourcePhases: [
            SourcePhaseDefinition(
                phase: .burning,
                range: CycleRange(startMilliseconds: 0, endMilliseconds: 30_000)
            ),
            SourcePhaseDefinition(
                phase: .cooling,
                range: CycleRange(startMilliseconds: 30_000, endMilliseconds: 90_000)
            ),
            SourcePhaseDefinition(
                phase: .stabilizing,
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000)
            ),
            SourcePhaseDefinition(
                phase: .stable,
                range: CycleRange(startMilliseconds: 690_000, endMilliseconds: 3_240_000)
            ),
        ],
        missionBands: [
            MissionBandDefinition(
                phase: .fireWave,
                range: CycleRange(startMilliseconds: 0, endMilliseconds: 30_000),
                title: "FIRE WAVE",
                threatInstruction: "Remain sheltered",
                heroEventID: "mission.coolingBegins"
            ),
            MissionBandDefinition(
                phase: .cooling,
                range: CycleRange(startMilliseconds: 30_000, endMilliseconds: 90_000),
                title: "COOLING",
                threatInstruction: "Remain sheltered",
                heroEventID: "mission.recoveryBegins"
            ),
            MissionBandDefinition(
                phase: .recovery,
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "QUIET WINDOW",
                threatInstruction: "Lower ambient risk; triggered encounters may persist",
                heroEventID: "enemy.expectedReturn"
            ),
            MissionBandDefinition(
                phase: .worldActive,
                range: CycleRange(startMilliseconds: 690_000, endMilliseconds: 3_090_000),
                title: "WORLD ACTIVE",
                threatInstruction: "Ordinary hostiles expected",
                heroEventID: "ruptura.warningBegins"
            ),
            MissionBandDefinition(
                phase: .rupturaWarning,
                range: CycleRange(startMilliseconds: 3_090_000, endMilliseconds: 3_240_000),
                title: "RUPTURA WARNING",
                threatInstruction: "Return, bank loot, and shelter",
                heroEventID: "ruptura.impact"
            ),
        ],
        events: [
            TimelineEventDefinition(
                id: "ruptura.impact", offsetMilliseconds: 0, category: .ruptura,
                title: "FIRE WAVE", target: "Ruptura impact", severity: .critical, isAlert: true
            ),
            TimelineEventDefinition(
                id: "mission.coolingBegins", offsetMilliseconds: 30_000, category: .phase,
                title: "COOLING", target: "Cooling begins", severity: .informational, isAlert: false
            ),
            TimelineEventDefinition(
                id: "mission.recoveryBegins", offsetMilliseconds: 90_000, category: .phase,
                title: "RECOVERY", target: "Lower-threat recovery begins", severity: .notice, isAlert: false
            ),
            TimelineEventDefinition(
                id: "enemy.finishOrLeave", offsetMilliseconds: 390_000, category: .enemy,
                title: "FINISH OR LEAVE", target: "5 minutes to expected Vermin return", severity: .notice, isAlert: true
            ),
            TimelineEventDefinition(
                id: "enemy.retreat", offsetMilliseconds: 570_000, category: .enemy,
                title: "RETREAT WINDOW", target: "2 minutes to expected Vermin return", severity: .warning, isAlert: true
            ),
            TimelineEventDefinition(
                id: "enemy.oneMinute", offsetMilliseconds: 630_000, category: .enemy,
                title: "ONE MINUTE", target: "1 minute to expected Vermin return", severity: .urgent, isAlert: true
            ),
            TimelineEventDefinition(
                id: "enemy.final15", offsetMilliseconds: 675_000, category: .enemy,
                title: "VERMIN IMMINENT", target: "15 seconds to expected Vermin return", severity: .urgent, isAlert: true
            ),
            TimelineEventDefinition(
                id: "enemy.expectedReturn", offsetMilliseconds: 690_000, category: .enemy,
                title: "HOSTILES ACTIVE", target: "Ordinary Vermin expected to return", severity: .critical, isAlert: true
            ),
            TimelineEventDefinition(
                id: "mission.worldActiveBegins", offsetMilliseconds: 690_000, category: .phase,
                title: "WORLD ACTIVE", target: "Normal ecology resumes", severity: .notice, isAlert: false
            ),
            TimelineEventDefinition(
                id: "ruptura.plan10", offsetMilliseconds: 2_640_000, category: .ruptura,
                title: "PLAN YOUR RETURN", target: "10 minutes to Ruptura", severity: .notice, isAlert: true
            ),
            TimelineEventDefinition(
                id: "ruptura.finishTrip5", offsetMilliseconds: 2_940_000, category: .ruptura,
                title: "FINISH THE TRIP", target: "5 minutes to Ruptura", severity: .warning, isAlert: true
            ),
            TimelineEventDefinition(
                id: "ruptura.warningBegins", offsetMilliseconds: 3_090_000, category: .ruptura,
                title: "RUPTURA WARNING", target: "2 minutes 30 seconds to impact", severity: .urgent, isAlert: true
            ),
            TimelineEventDefinition(
                id: "ruptura.oneMinute", offsetMilliseconds: 3_180_000, category: .ruptura,
                title: "SHELTER NOW", target: "1 minute to Ruptura", severity: .urgent, isAlert: true
            ),
            TimelineEventDefinition(
                id: "ruptura.thirtySeconds", offsetMilliseconds: 3_210_000, category: .ruptura,
                title: "SHELTER NOW", target: "30 seconds to Ruptura", severity: .critical, isAlert: true
            ),
            TimelineEventDefinition(
                id: "ruptura.final15", offsetMilliseconds: 3_225_000, category: .ruptura,
                title: "IMPACT IMMINENT", target: "15 seconds to Ruptura", severity: .critical, isAlert: true
            ),
        ],
        opportunities: [
            OpportunityDefinition(
                id: "poi.lowerThreat",
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "Lower ambient POI threat",
                action: "Use the recovery window",
                caveat: "Triggered and scripted encounters may persist",
                evidenceClass: .communityDerived,
                confidence: .medium,
                defaultRank: 0
            ),
            OpportunityDefinition(
                id: "resource.starTears",
                range: CycleRange(startMilliseconds: 570_000, endMilliseconds: 690_000),
                title: "Star Tears forming",
                action: "Leave some cooled Ignitium if Star Tears are the priority",
                caveat: "Exact timing remains experimental",
                evidenceClass: .experimental,
                confidence: .experimental,
                defaultRank: 5
            ),
            OpportunityDefinition(
                id: "resource.oxallop",
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "Dry lakebeds exposed",
                action: "Gather Oxallop before water returns",
                caveat: "Availability timing is community-derived",
                evidenceClass: .communityDerived,
                confidence: .medium,
                defaultRank: 10
            ),
            OpportunityDefinition(
                id: "resource.caves",
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "Caves favorable now",
                action: "Check for Quartz and Glowcaps",
                caveat: "Access is favorable; respawn is not guaranteed",
                evidenceClass: .communityDerived,
                confidence: .medium,
                defaultRank: 20
            ),
            OpportunityDefinition(
                id: "resource.ignitium",
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "Fresh Ignitium window",
                action: "Search post-wave terrain",
                caveat: "Spawn rate may vary",
                evidenceClass: .communityDerived,
                confidence: .medium,
                defaultRank: 30
            ),
            OpportunityDefinition(
                id: "wildlife.eggs",
                range: CycleRange(startMilliseconds: 90_000, endMilliseconds: 690_000),
                title: "Post-wave egg opportunity",
                action: "Check Coralion and Vulpir habitats",
                caveat: "Spawns are variable",
                evidenceClass: .communityDerived,
                confidence: .medium,
                defaultRank: 40
            ),
            OpportunityDefinition(
                id: "world.regrowth",
                range: CycleRange(startMilliseconds: 690_000, endMilliseconds: 3_240_000),
                title: "Surface regrowth",
                action: "Foliage and water are expected to be restored",
                caveat: "No species-specific timing guarantee",
                evidenceClass: .official,
                confidence: .high,
                defaultRank: 50
            ),
        ]
    )
}
