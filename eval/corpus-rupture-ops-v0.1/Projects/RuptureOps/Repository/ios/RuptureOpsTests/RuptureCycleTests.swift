import XCTest
@testable import RuptureOps

final class RuptureCycleTests: XCTestCase {
    private let cycle = RuptureCycle.current

    func testCurrentDefinitionCarriesSeparateVersionAxesAndValidates() throws {
        try cycle.definition.validate()
        XCTAssertEqual(cycle.version.definitionID, "ruptura-cycle/u1-srdb-2.3.4/r1")
        XCTAssertEqual(
            cycle.version.sourceSnapshotID,
            "ea-u1-gv35118973__srdb-2.3.4__20260711T050230Z"
        )
        XCTAssertEqual(
            cycle.version.sourceClaimedGameBuildID,
            "ea_u1_0.2.0_build_22674441"
        )
        XCTAssertEqual(
            cycle.version.latestPublicBuildAtCaptureID,
            "ea_u1_0.2.8_build_23761620"
        )
        XCTAssertEqual(cycle.cycleLengthMilliseconds, 3_240_000)
    }

    func testDefinitionRejectsCoverageGap() {
        let original = cycle.definition
        let invalid = RuptureCycleDefinition(
            version: original.version,
            cycleLengthMilliseconds: original.cycleLengthMilliseconds,
            sourcePhases: Array(original.sourcePhases.dropLast()),
            missionBands: original.missionBands,
            events: original.events,
            opportunities: original.opportunities
        )
        XCTAssertThrowsError(try invalid.validate()) { error in
            XCTAssertEqual(
                error as? CycleDefinitionValidationError,
                .invalidCoverage("source")
            )
        }
    }

    func testDefinitionRejectsDuplicateEventID() {
        let original = cycle.definition
        let invalid = RuptureCycleDefinition(
            version: original.version,
            cycleLengthMilliseconds: original.cycleLengthMilliseconds,
            sourcePhases: original.sourcePhases,
            missionBands: original.missionBands,
            events: original.events + [original.events[0]],
            opportunities: original.opportunities
        )
        XCTAssertThrowsError(try invalid.validate()) { error in
            XCTAssertEqual(
                error as? CycleDefinitionValidationError,
                .duplicateID("ruptura.impact")
            )
        }
    }

    func testDefinitionRejectsOutOfRangeEvent() {
        let original = cycle.definition
        let invalidEvent = TimelineEventDefinition(
            id: "invalid",
            offsetMilliseconds: original.cycleLengthMilliseconds,
            category: .phase,
            title: "Invalid",
            target: "Invalid",
            severity: .informational,
            isAlert: false
        )
        let invalid = RuptureCycleDefinition(
            version: original.version,
            cycleLengthMilliseconds: original.cycleLengthMilliseconds,
            sourcePhases: original.sourcePhases,
            missionBands: original.missionBands,
            events: original.events + [invalidEvent],
            opportunities: original.opportunities
        )
        XCTAssertThrowsError(try invalid.validate()) { error in
            XCTAssertEqual(
                error as? CycleDefinitionValidationError,
                .invalidEventOffset("invalid")
            )
        }
    }

    func testFloorNormalizationAtPositiveAndNegativeCycleEdges() {
        let cases: [(CycleMilliseconds, Int64, CycleMilliseconds)] = [
            (0, 0, 0),
            (1, 0, 1),
            (3_239_999, 0, 3_239_999),
            (3_240_000, 1, 0),
            (3_240_001, 1, 1),
            (6_480_123, 2, 123),
            (-1, -1, 3_239_999),
            (-3_239_999, -1, 1),
            (-3_240_000, -1, 0),
            (-3_240_001, -2, 3_239_999),
            (-6_479_877, -2, 123),
        ]

        for (elapsed, expectedIndex, expectedOffset) in cases {
            let coordinate = cycle.coordinate(elapsedMilliseconds: elapsed)
            XCTAssertEqual(coordinate.cycleIndex, expectedIndex, "elapsed \(elapsed)")
            XCTAssertEqual(coordinate.offsetMilliseconds, expectedOffset, "elapsed \(elapsed)")
        }
    }

    func testSourcePhaseHalfOpenBoundaries() {
        let cases: [(CycleMilliseconds, SourcePhase, CycleMilliseconds)] = [
            (0, .burning, 30_000),
            (29_999, .burning, 1),
            (30_000, .cooling, 60_000),
            (89_999, .cooling, 1),
            (90_000, .stabilizing, 600_000),
            (689_999, .stabilizing, 1),
            (690_000, .stable, 2_550_000),
            (3_239_999, .stable, 1),
            (3_240_000, .burning, 30_000),
        ]

        for (elapsed, phase, remaining) in cases {
            let snapshot = cycle.snapshot(elapsedMilliseconds: elapsed)
            XCTAssertEqual(snapshot.sourcePhase, phase, "elapsed \(elapsed)")
            XCTAssertEqual(
                snapshot.sourcePhaseRemainingMilliseconds,
                remaining,
                "elapsed \(elapsed)"
            )
        }
    }

    func testMissionPhaseHalfOpenBoundaries() {
        let cases: [(CycleMilliseconds, MissionPhase, CycleMilliseconds)] = [
            (0, .fireWave, 30_000),
            (29_999, .fireWave, 1),
            (30_000, .cooling, 60_000),
            (89_999, .cooling, 1),
            (90_000, .recovery, 600_000),
            (689_999, .recovery, 1),
            (690_000, .worldActive, 2_400_000),
            (3_089_999, .worldActive, 1),
            (3_090_000, .rupturaWarning, 150_000),
            (3_239_999, .rupturaWarning, 1),
            (3_240_000, .fireWave, 30_000),
        ]

        for (elapsed, phase, remaining) in cases {
            let snapshot = cycle.snapshot(elapsedMilliseconds: elapsed)
            XCTAssertEqual(snapshot.phase, phase, "elapsed \(elapsed)")
            XCTAssertEqual(
                snapshot.missionPhaseRemainingMilliseconds,
                remaining,
                "elapsed \(elapsed)"
            )
        }
    }

    func testSourceStableAndMissionWarningRemainSeparate() {
        let snapshot = cycle.snapshot(elapsedMilliseconds: 3_090_000)
        XCTAssertEqual(snapshot.sourcePhase, .stable)
        XCTAssertEqual(snapshot.phase, .rupturaWarning)
    }

    func testHeroTargetAdvancesAtEveryExactBoundary() {
        let cases: [(CycleMilliseconds, String, CycleMilliseconds)] = [
            (0, "Cooling begins", 30_000),
            (30_000, "Lower-threat recovery begins", 60_000),
            (90_000, "Ordinary Vermin expected to return", 600_000),
            (690_000, "2 minutes 30 seconds to impact", 2_400_000),
            (3_090_000, "Ruptura impact", 150_000),
            (3_240_000, "Cooling begins", 30_000),
        ]

        for (elapsed, target, remaining) in cases {
            let snapshot = cycle.snapshot(elapsedMilliseconds: elapsed)
            XCTAssertEqual(snapshot.target, target, "elapsed \(elapsed)")
            XCTAssertEqual(snapshot.heroRemainingMilliseconds, remaining, "elapsed \(elapsed)")
        }
    }

    func testRecoveryOpportunityBoundariesAndOrdering() {
        XCTAssertTrue(cycle.snapshot(elapsedMilliseconds: 89_999).opportunities.isEmpty)

        let early = cycle.snapshot(elapsedMilliseconds: 90_000).opportunities
        XCTAssertEqual(
            early.map(\.id),
            [
                "poi.lowerThreat",
                "resource.oxallop",
                "resource.caves",
                "resource.ignitium",
                "wildlife.eggs",
            ]
        )
        XCTAssertEqual(early[0].remainingMilliseconds, 600_000)

        let late = cycle.snapshot(elapsedMilliseconds: 570_000).opportunities
        XCTAssertEqual(late[0].id, "poi.lowerThreat")
        XCTAssertEqual(late[1].id, "resource.starTears")
        XCTAssertEqual(late[1].remainingMilliseconds, 120_000)

        XCTAssertEqual(
            cycle.snapshot(elapsedMilliseconds: 689_999).opportunities.count,
            6
        )
        XCTAssertEqual(
            cycle.snapshot(elapsedMilliseconds: 690_000).opportunities.map(\.id),
            ["world.regrowth"]
        )
        XCTAssertEqual(
            cycle.snapshot(elapsedMilliseconds: 3_239_999).opportunities[0]
                .remainingMilliseconds,
            1
        )
        XCTAssertTrue(cycle.snapshot(elapsedMilliseconds: 3_240_000).opportunities.isEmpty)
    }

    func testOccurrencesUseOpenStartAndClosedEnd() {
        XCTAssertEqual(
            cycle.occurrences(after: 389_999, through: 390_000).map { $0.event.id },
            ["enemy.finishOrLeave"]
        )
        XCTAssertTrue(cycle.occurrences(after: 390_000, through: 390_001).isEmpty)
    }

    func testSameInstantEventsAreDeterministicallyOrdered() {
        let occurrences = cycle.occurrences(after: 689_999, through: 690_000)
        XCTAssertEqual(
            occurrences.map { $0.event.id },
            ["enemy.expectedReturn", "mission.worldActiveBegins"]
        )
    }

    func testOccurrenceEnumerationWrapsImpactIntoNextCycle() {
        let occurrences = cycle.occurrences(after: 3_239_999, through: 3_240_000)
        XCTAssertEqual(occurrences.count, 1)
        XCTAssertEqual(occurrences[0].event.id, "ruptura.impact")
        XCTAssertEqual(occurrences[0].cycleIndex, 1)
        XCTAssertEqual(occurrences[0].remainingMilliseconds, 1)
    }

    func testOneCycleHorizonContainsOneOccurrenceOfEveryEvent() {
        let occurrences = cycle.occurrences(after: 0, through: 3_240_000)
        XCTAssertEqual(occurrences.count, cycle.definition.events.count)
        XCTAssertEqual(occurrences.last?.event.id, "ruptura.impact")
        XCTAssertEqual(occurrences.last?.cycleIndex, 1)
    }

    func testOccurrenceEnumerationHandlesMultipleCyclesAndBackwardIntervals() {
        let occurrences = cycle.occurrences(after: 3_239_999, through: 6_480_000)
        XCTAssertEqual(occurrences.filter { $0.event.id == "ruptura.impact" }.count, 2)
        XCTAssertTrue(cycle.occurrences(after: 1_000, through: 999).isEmpty)
        XCTAssertTrue(cycle.occurrences(after: 1_000, through: 1_000).isEmpty)
    }

    func testSnapshotTimelineIsStrictlyFutureAndOneCycleLong() {
        let snapshot = cycle.snapshot(elapsedMilliseconds: 690_000)
        XCTAssertTrue(snapshot.timelineEvents.allSatisfy { $0.remainingMilliseconds > 0 })
        XCTAssertEqual(snapshot.timelineEvents.count, cycle.definition.events.count)
        XCTAssertEqual(snapshot.timelineEvents.last?.absoluteMilliseconds, 3_930_000)
    }

    func testFractionalSecondsAreFlooredBeforeBoundaryLookup() {
        XCTAssertEqual(cycle.snapshot(elapsed: 29.9999).phase, .fireWave)
        XCTAssertEqual(cycle.snapshot(elapsed: 30.0001).phase, .cooling)
        XCTAssertEqual(cycle.coordinate(elapsed: -0.0001).cycleIndex, -1)
        XCTAssertEqual(cycle.coordinate(elapsed: -0.0001).offsetMilliseconds, 3_239_999)
    }
}
