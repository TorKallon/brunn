import Foundation

public enum SyncCue: String, Codable, CaseIterable, Sendable {
    case fireWaveHitNow
    case coolingBeganNow
    case recoveryBeganNow
    case worldActiveBeganNow
    case warningBeganNow

    public var offsetMilliseconds: CycleMilliseconds {
        switch self {
        case .fireWaveHitNow: 0
        case .coolingBeganNow: 30_000
        case .recoveryBeganNow: 90_000
        case .worldActiveBeganNow: 690_000
        case .warningBeganNow: 3_090_000
        }
    }
}

public enum SyncConfidence: Codable, Equatable, Sendable {
    case manualPredicted
    case confirmedAtTransition(cue: SyncCue, correctionMilliseconds: CycleMilliseconds)
    case driftSuspected
    case modelChangedRequiresResync
}

public struct WatchSession: Codable, Equatable, Sendable {
    public private(set) var anchor: Date
    public private(set) var pausedElapsedMilliseconds: CycleMilliseconds?
    public private(set) var mutedCycle: Int64?
    public private(set) var cycleDefinitionID: String
    public private(set) var syncEpochID: UUID
    public private(set) var confidence: SyncConfidence

    public init(
        anchor: Date,
        cycleDefinitionID: String = RuptureCycle.current.version.definitionID,
        syncEpochID: UUID = UUID(),
        confidence: SyncConfidence = .manualPredicted
    ) {
        self.anchor = anchor
        self.pausedElapsedMilliseconds = nil
        self.mutedCycle = nil
        self.cycleDefinitionID = cycleDefinitionID
        self.syncEpochID = syncEpochID
        self.confidence = confidence
    }

    public init(
        synchronizedAt date: Date,
        cue: SyncCue,
        cycle: RuptureCycle = .current,
        syncEpochID: UUID = UUID()
    ) {
        self.init(
            anchor: date.addingTimeInterval(
                -TimeInterval(cue.offsetMilliseconds) / 1_000
            ),
            cycleDefinitionID: cycle.version.definitionID,
            syncEpochID: syncEpochID,
            confidence: .manualPredicted
        )
    }

    public var isPaused: Bool {
        pausedElapsedMilliseconds != nil
    }

    public func elapsed(at now: Date) -> TimeInterval {
        TimeInterval(elapsedMilliseconds(at: now)) / 1_000
    }

    public func elapsedMilliseconds(at now: Date) -> CycleMilliseconds {
        if let pausedElapsedMilliseconds {
            return pausedElapsedMilliseconds
        }
        return RuptureCycle.milliseconds(from: now.timeIntervalSince(anchor))
    }

    public func snapshot(
        at now: Date,
        cycle: RuptureCycle = .current
    ) -> CycleSnapshot {
        precondition(
            cycleDefinitionID == cycle.version.definitionID,
            "Watch session requires a different cycle definition"
        )
        return cycle.snapshot(elapsedMilliseconds: elapsedMilliseconds(at: now))
    }

    public mutating func pause(at now: Date) {
        guard pausedElapsedMilliseconds == nil else { return }
        pausedElapsedMilliseconds = elapsedMilliseconds(at: now)
    }

    public mutating func resume(at now: Date) {
        guard let pausedElapsedMilliseconds else { return }
        anchor = now.addingTimeInterval(
            -TimeInterval(pausedElapsedMilliseconds) / 1_000
        )
        self.pausedElapsedMilliseconds = nil
    }

    /// Positive adjustments advance the prediction; negative adjustments rewind it.
    public mutating func adjust(by seconds: TimeInterval, at now: Date) {
        let adjustment = RuptureCycle.milliseconds(from: seconds)
        if let pausedElapsedMilliseconds {
            self.pausedElapsedMilliseconds = adding(
                adjustment,
                to: pausedElapsedMilliseconds
            )
        } else {
            anchor = anchor.addingTimeInterval(-TimeInterval(adjustment) / 1_000)
        }
        confidence = .manualPredicted
    }

    public mutating func resync(
        at now: Date,
        cue: SyncCue = .fireWaveHitNow,
        cycle: RuptureCycle = .current,
        syncEpochID: UUID = UUID()
    ) {
        anchor = now.addingTimeInterval(
            -TimeInterval(cue.offsetMilliseconds) / 1_000
        )
        pausedElapsedMilliseconds = nil
        mutedCycle = nil
        cycleDefinitionID = cycle.version.definitionID
        self.syncEpochID = syncEpochID
        confidence = .manualPredicted
    }

    public mutating func confirm(
        _ cue: SyncCue,
        at now: Date,
        cycle: RuptureCycle = .current
    ) {
        let before = elapsedMilliseconds(at: now)
        let coordinate = cycle.coordinate(elapsedMilliseconds: before)
        let correction = shortestCorrection(
            from: coordinate.offsetMilliseconds,
            to: cue.offsetMilliseconds,
            cycleLength: cycle.cycleLengthMilliseconds
        )
        let (snappedElapsed, overflow) = before.addingReportingOverflow(correction)
        precondition(!overflow, "Watch session confirmation overflow")
        anchor = now.addingTimeInterval(
            -TimeInterval(snappedElapsed) / 1_000
        )
        pausedElapsedMilliseconds = nil
        mutedCycle = nil
        cycleDefinitionID = cycle.version.definitionID
        confidence = .confirmedAtTransition(
            cue: cue,
            correctionMilliseconds: correction
        )
    }

    @discardableResult
    public mutating func muteCurrentCycle(
        at now: Date,
        cycle: RuptureCycle = .current
    ) -> Int64 {
        let currentCycle = cycle.coordinate(
            elapsedMilliseconds: elapsedMilliseconds(at: now)
        ).cycleIndex
        mutedCycle = currentCycle
        return currentCycle
    }

    public mutating func clearMute() {
        mutedCycle = nil
    }

    public mutating func markDriftSuspected() {
        confidence = .driftSuspected
    }

    public mutating func requireResync(forDefinitionID definitionID: String) {
        guard cycleDefinitionID != definitionID else { return }
        confidence = .modelChangedRequiresResync
    }

    public func plannedAlerts(
        at now: Date,
        horizonCycles: Int = 1,
        cycle: RuptureCycle = .current
    ) -> [AlertOccurrence] {
        guard !isPaused else { return [] }
        return AlertPlanner.occurrences(
            anchor: anchor,
            now: now,
            horizonCycles: horizonCycles,
            mutedCycle: mutedCycle,
            cycle: cycle,
            syncEpochID: syncEpochID
        )
    }

    private func adding(
        _ value: CycleMilliseconds,
        to base: CycleMilliseconds
    ) -> CycleMilliseconds {
        let (result, overflow) = base.addingReportingOverflow(value)
        precondition(!overflow, "Watch session elapsed time overflow")
        return result
    }

    private func shortestCorrection(
        from current: CycleMilliseconds,
        to target: CycleMilliseconds,
        cycleLength: CycleMilliseconds
    ) -> CycleMilliseconds {
        var correction = target - current
        let halfCycle = cycleLength / 2
        if correction > halfCycle {
            correction -= cycleLength
        } else if correction < -halfCycle {
            correction += cycleLength
        }
        return correction
    }
}
