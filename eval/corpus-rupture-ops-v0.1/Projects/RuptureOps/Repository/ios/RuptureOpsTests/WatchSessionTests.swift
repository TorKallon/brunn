import XCTest
@testable import RuptureOps

final class WatchSessionTests: XCTestCase {
    private let cycle = RuptureCycle.current
    private let zero = Date(timeIntervalSinceReferenceDate: 1_000_000)

    func testAnchorIsCycleZeroImpact() {
        let session = WatchSession(anchor: zero)
        XCTAssertEqual(session.elapsed(at: zero), 0)
        XCTAssertEqual(session.elapsed(at: zero.addingTimeInterval(90)), 90)
        XCTAssertEqual(
            session.snapshot(at: zero.addingTimeInterval(90)).phase,
            .recovery
        )
    }

    func testEverySyncCueProducesItsExactOffset() {
        for cue in SyncCue.allCases {
            let session = WatchSession(synchronizedAt: zero, cue: cue)
            XCTAssertEqual(
                session.elapsedMilliseconds(at: zero),
                cue.offsetMilliseconds,
                "cue \(cue)"
            )
        }
    }

    func testPauseFreezesElapsedTimeAndIsIdempotent() {
        var session = WatchSession(anchor: zero)
        let pauseDate = zero.addingTimeInterval(100)
        session.pause(at: pauseDate)
        session.pause(at: zero.addingTimeInterval(200))

        XCTAssertTrue(session.isPaused)
        XCTAssertEqual(session.elapsed(at: pauseDate), 100)
        XCTAssertEqual(session.elapsed(at: zero.addingTimeInterval(10_000)), 100)
    }

    func testResumeDoesNotAddPausedWallTimeAndIsIdempotent() {
        var session = WatchSession(anchor: zero)
        session.pause(at: zero.addingTimeInterval(100))
        session.resume(at: zero.addingTimeInterval(1_000))
        session.resume(at: zero.addingTimeInterval(2_000))

        XCTAssertFalse(session.isPaused)
        XCTAssertEqual(session.elapsed(at: zero.addingTimeInterval(1_000)), 100)
        XCTAssertEqual(session.elapsed(at: zero.addingTimeInterval(1_010)), 110)
    }

    func testRunningAdjustmentAdvancesAndRewindsPrediction() {
        var session = WatchSession(anchor: zero)
        let now = zero.addingTimeInterval(100)

        session.adjust(by: 5, at: now)
        XCTAssertEqual(session.elapsed(at: now), 105)

        session.adjust(by: -6, at: now)
        XCTAssertEqual(session.elapsed(at: now), 99)
        XCTAssertEqual(session.confidence, .manualPredicted)
    }

    func testPausedAdjustmentChangesFrozenPositionWithoutResuming() {
        var session = WatchSession(anchor: zero)
        let now = zero.addingTimeInterval(100)
        session.pause(at: now)
        session.adjust(by: 5, at: zero.addingTimeInterval(1_000))

        XCTAssertTrue(session.isPaused)
        XCTAssertEqual(session.elapsed(at: zero.addingTimeInterval(2_000)), 105)
    }

    func testNegativeAdjustmentUsesFloorNormalizedSnapshot() {
        var session = WatchSession(anchor: zero)
        session.adjust(by: -1, at: zero)
        let snapshot = session.snapshot(at: zero)

        XCTAssertEqual(snapshot.cycleIndex, -1)
        XCTAssertEqual(snapshot.elapsedInCycleMilliseconds, 3_239_000)
        XCTAssertEqual(snapshot.phase, .rupturaWarning)
    }

    func testResyncCreatesNewEpochClearsPauseAndMute() {
        let originalEpoch = UUID(uuidString: "11111111-1111-1111-1111-111111111111")!
        let replacementEpoch = UUID(uuidString: "22222222-2222-2222-2222-222222222222")!
        var session = WatchSession(
            synchronizedAt: zero,
            cue: .fireWaveHitNow,
            syncEpochID: originalEpoch
        )
        session.pause(at: zero.addingTimeInterval(100))
        session.muteCurrentCycle(at: zero.addingTimeInterval(100))

        session.resync(
            at: zero.addingTimeInterval(500),
            cue: .recoveryBeganNow,
            syncEpochID: replacementEpoch
        )

        XCTAssertFalse(session.isPaused)
        XCTAssertNil(session.mutedCycle)
        XCTAssertEqual(session.syncEpochID, replacementEpoch)
        XCTAssertEqual(session.elapsedMilliseconds(at: zero.addingTimeInterval(500)), 90_000)
    }

    func testConfirmSnapsToNearestObservedTransitionAndPreservesCycleProgress() {
        var firstCycle = WatchSession(anchor: zero)
        let coolingObservation = zero.addingTimeInterval(29)
        firstCycle.confirm(.coolingBeganNow, at: coolingObservation)
        XCTAssertEqual(firstCycle.elapsed(at: coolingObservation), 30)
        XCTAssertEqual(
            firstCycle.confidence,
            .confirmedAtTransition(cue: .coolingBeganNow, correctionMilliseconds: 1_000)
        )

        var nextCycle = WatchSession(anchor: zero)
        let impactObservation = zero.addingTimeInterval(3_239)
        nextCycle.confirm(.fireWaveHitNow, at: impactObservation)
        let snapshot = nextCycle.snapshot(at: impactObservation)
        XCTAssertEqual(snapshot.cycleIndex, 1)
        XCTAssertEqual(snapshot.elapsedInCycleMilliseconds, 0)
        XCTAssertEqual(
            nextCycle.confidence,
            .confirmedAtTransition(cue: .fireWaveHitNow, correctionMilliseconds: 1_000)
        )
    }

    func testMuteCurrentCycleTracksSignedCycleIndexAndCanClear() {
        var session = WatchSession(anchor: zero)
        XCTAssertEqual(
            session.muteCurrentCycle(at: zero.addingTimeInterval(700)),
            0
        )
        XCTAssertEqual(session.mutedCycle, 0)
        session.clearMute()
        XCTAssertNil(session.mutedCycle)

        session.adjust(by: -4_000, at: zero)
        XCTAssertEqual(session.muteCurrentCycle(at: zero), -2)
    }

    func testPausedSessionPlansNoAlerts() {
        var session = WatchSession(anchor: zero)
        session.pause(at: zero.addingTimeInterval(100))
        XCTAssertTrue(session.plannedAlerts(at: zero.addingTimeInterval(200)).isEmpty)
    }

    func testDefinitionChangeMarksSessionAsRequiringResync() {
        var session = WatchSession(anchor: zero)
        session.requireResync(forDefinitionID: session.cycleDefinitionID)
        XCTAssertEqual(session.confidence, .manualPredicted)

        session.requireResync(forDefinitionID: "future-definition")
        XCTAssertEqual(session.confidence, .modelChangedRequiresResync)
    }

    func testCodableRoundTripPreservesPausedStateAndVersion() throws {
        var session = WatchSession(anchor: zero)
        session.pause(at: zero.addingTimeInterval(123))
        session.muteCurrentCycle(at: zero.addingTimeInterval(123))

        let data = try JSONEncoder().encode(session)
        let decoded = try JSONDecoder().decode(WatchSession.self, from: data)
        XCTAssertEqual(decoded, session)
        XCTAssertEqual(decoded.elapsed(at: zero.addingTimeInterval(999)), 123)
        XCTAssertEqual(decoded.cycleDefinitionID, cycle.version.definitionID)
    }
}
