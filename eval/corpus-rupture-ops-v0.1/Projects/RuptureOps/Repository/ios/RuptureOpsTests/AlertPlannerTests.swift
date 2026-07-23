import XCTest
@testable import RuptureOps

final class AlertPlannerTests: XCTestCase {
    private let cycle = RuptureCycle.current
    private let anchor = Date(timeIntervalSinceReferenceDate: 2_000_000)
    private let epoch = UUID(uuidString: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")!

    func testOneCycleHorizonPlansEveryAlertExactlyOnce() {
        let alerts = AlertPlanner.occurrences(
            anchor: anchor,
            now: anchor,
            horizonCycles: 1,
            cycle: cycle,
            syncEpochID: epoch
        )

        XCTAssertEqual(alerts.count, cycle.definition.events.filter(\.isAlert).count)
        XCTAssertEqual(alerts.first?.event.id, "enemy.finishOrLeave")
        XCTAssertEqual(alerts.first?.date, anchor.addingTimeInterval(390))
        XCTAssertEqual(alerts.last?.event.id, "ruptura.impact")
        XCTAssertEqual(alerts.last?.cycleIndex, 1)
        XCTAssertEqual(alerts.last?.date, anchor.addingTimeInterval(3_240))
    }

    func testSchedulingIsStrictlyAfterNowAtExactThreshold() {
        let now = anchor.addingTimeInterval(390)
        let alerts = AlertPlanner.occurrences(
            anchor: anchor,
            now: now,
            horizonCycles: 1,
            cycle: cycle,
            syncEpochID: epoch
        )
        let finishOrLeave = alerts.first { $0.event.id == "enemy.finishOrLeave" }

        XCTAssertEqual(finishOrLeave?.cycleIndex, 1)
        XCTAssertEqual(finishOrLeave?.date, anchor.addingTimeInterval(3_630))
    }

    func testZeroOrNegativeHorizonPlansNothing() {
        XCTAssertTrue(
            AlertPlanner.occurrences(
                anchor: anchor, now: anchor, horizonCycles: 0, cycle: cycle
            ).isEmpty
        )
        XCTAssertTrue(
            AlertPlanner.occurrences(
                anchor: anchor, now: anchor, horizonCycles: -1, cycle: cycle
            ).isEmpty
        )
    }

    func testMuteCurrentCycleSuppressesRemainderAndNextImpactInclusive() {
        let alerts = AlertPlanner.occurrences(
            anchor: anchor,
            now: anchor,
            horizonCycles: 1,
            mutedCycle: 0,
            cycle: cycle,
            syncEpochID: epoch
        )
        XCTAssertTrue(alerts.isEmpty)

        XCTAssertTrue(
            AlertPlanner.isMuted(
                absoluteMilliseconds: 3_240_000,
                mutedCycle: 0,
                cycleLength: cycle.cycleLengthMilliseconds
            )
        )
        XCTAssertFalse(
            AlertPlanner.isMuted(
                absoluteMilliseconds: 3_240_001,
                mutedCycle: 0,
                cycleLength: cycle.cycleLengthMilliseconds
            )
        )
    }

    func testAlertsResumeImmediatelyAfterMutedImpact() {
        let now = anchor.addingTimeInterval(3_240)
        let alerts = AlertPlanner.occurrences(
            anchor: anchor,
            now: now,
            horizonCycles: 1,
            mutedCycle: 0,
            cycle: cycle,
            syncEpochID: epoch
        )

        XCTAssertEqual(alerts.first?.event.id, "enemy.finishOrLeave")
        XCTAssertEqual(alerts.first?.cycleIndex, 1)
        XCTAssertEqual(alerts.first?.date, anchor.addingTimeInterval(3_630))
    }

    func testDeterministicIdentifierSeparatesEpochEventAndCycle() {
        let first = AlertPlanner.identifier(
            syncEpochID: epoch,
            definitionID: cycle.version.definitionID,
            eventID: "enemy.finishOrLeave",
            cycleIndex: 0
        )
        let nextCycle = AlertPlanner.identifier(
            syncEpochID: epoch,
            definitionID: cycle.version.definitionID,
            eventID: "enemy.finishOrLeave",
            cycleIndex: 1
        )
        let nextEpoch = AlertPlanner.identifier(
            syncEpochID: UUID(uuidString: "BBBBBBBB-BBBB-CCCC-DDDD-EEEEEEEEEEEE")!,
            definitionID: cycle.version.definitionID,
            eventID: "enemy.finishOrLeave",
            cycleIndex: 0
        )

        XCTAssertNotEqual(first, nextCycle)
        XCTAssertNotEqual(first, nextEpoch)
        XCTAssertTrue(first.contains(cycle.version.definitionID))
    }

    func testForegroundCrossingIncludesNewEndpointAndEmitsOnce() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 390_000,
            context: .foregroundTick,
            cycle: cycle
        )

        XCTAssertEqual(evaluation.crossedEvents.map { $0.event.id }, ["enemy.finishOrLeave"])
        XCTAssertEqual(evaluation.presentationEvents.map { $0.event.id }, ["enemy.finishOrLeave"])
        XCTAssertEqual(evaluation.missedCount, 0)
        XCTAssertFalse(evaluation.needsStateReconciliation)

        let noReplay = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 390_000,
            toMilliseconds: 390_001,
            context: .foregroundTick,
            cycle: cycle
        )
        XCTAssertTrue(noReplay.crossedEvents.isEmpty)
        XCTAssertTrue(noReplay.presentationEvents.isEmpty)
    }

    func testForegroundFreshnessBoundaryIsInclusiveAtFiveSeconds() {
        let fresh = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 395_000,
            context: .foregroundTick,
            cycle: cycle
        )
        XCTAssertEqual(fresh.presentationEvents.map { $0.event.id }, ["enemy.finishOrLeave"])

        let stale = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 395_001,
            context: .foregroundTick,
            cycle: cycle
        )
        XCTAssertTrue(stale.presentationEvents.isEmpty)
        XCTAssertEqual(stale.missedCount, 1)
        XCTAssertTrue(stale.needsStateReconciliation)
    }

    func testDelayedForegroundTickCollapsesToLatestFreshThreshold() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 570_000,
            context: .foregroundTick,
            cycle: cycle
        )

        XCTAssertEqual(
            evaluation.crossedEvents.map { $0.event.id },
            ["enemy.finishOrLeave", "enemy.retreat"]
        )
        XCTAssertEqual(evaluation.presentationEvents.map { $0.event.id }, ["enemy.retreat"])
        XCTAssertEqual(evaluation.missedCount, 1)
    }

    func testResumeNeverReplaysHistoricalThresholds() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 700_000,
            context: .resume,
            cycle: cycle
        )

        XCTAssertEqual(evaluation.crossedEvents.count, 5)
        XCTAssertTrue(evaluation.presentationEvents.isEmpty)
        XCTAssertEqual(evaluation.missedCount, 5)
        XCTAssertTrue(evaluation.needsStateReconciliation)
    }

    func testClockCorrectionNeverReplaysEvenFreshEvent() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 390_000,
            context: .clockCorrection,
            cycle: cycle
        )
        XCTAssertTrue(evaluation.presentationEvents.isEmpty)
        XCTAssertEqual(evaluation.missedCount, 1)
        XCTAssertTrue(evaluation.needsStateReconciliation)
    }

    func testBackwardMovementProducesNoEventsAndRequiresReconciliation() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 700_000,
            toMilliseconds: 600_000,
            context: .foregroundTick,
            cycle: cycle
        )
        XCTAssertTrue(evaluation.crossedEvents.isEmpty)
        XCTAssertTrue(evaluation.presentationEvents.isEmpty)
        XCTAssertTrue(evaluation.needsStateReconciliation)
    }

    func testMutedCrossingIsConsumedWithoutPresentationOrReconciliation() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 389_999,
            toMilliseconds: 390_000,
            context: .foregroundTick,
            mutedCycle: 0,
            cycle: cycle
        )
        XCTAssertEqual(evaluation.crossedEvents.count, 1)
        XCTAssertTrue(evaluation.presentationEvents.isEmpty)
        XCTAssertEqual(evaluation.missedCount, 0)
        XCTAssertFalse(evaluation.needsStateReconciliation)
    }

    func testImpactCrossingBelongsToNextCycle() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 3_239_999,
            toMilliseconds: 3_240_000,
            context: .foregroundTick,
            cycle: cycle
        )
        XCTAssertEqual(evaluation.presentationEvents.first?.event.id, "ruptura.impact")
        XCTAssertEqual(evaluation.presentationEvents.first?.cycleIndex, 1)
    }

    func testLongResumeAcrossSeveralCyclesCollapsesWithoutBacklog() {
        let evaluation = AlertPlanner.evaluateCrossing(
            fromMilliseconds: 0,
            toMilliseconds: 10_000_000,
            context: .resume,
            cycle: cycle
        )
        XCTAssertGreaterThan(evaluation.crossedEvents.count, 20)
        XCTAssertTrue(evaluation.presentationEvents.isEmpty)
        XCTAssertTrue(evaluation.needsStateReconciliation)
    }

    func testWatchSessionPlannerUsesStableSyncEpochIdentifiers() {
        let session = WatchSession(
            synchronizedAt: anchor,
            cue: .fireWaveHitNow,
            syncEpochID: epoch
        )
        let first = session.plannedAlerts(at: anchor)
        let second = session.plannedAlerts(at: anchor.addingTimeInterval(1))

        let firstByEvent = Dictionary(uniqueKeysWithValues: first.map { ($0.event.id, $0.id) })
        let secondByEvent = Dictionary(uniqueKeysWithValues: second.map { ($0.event.id, $0.id) })
        XCTAssertEqual(
            firstByEvent["enemy.finishOrLeave"],
            secondByEvent["enemy.finishOrLeave"]
        )
    }
}
