import Foundation

public struct AlertOccurrence: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let event: TimelineEventDefinition
    public let cycleIndex: Int64
    public let date: Date
    public let absoluteMilliseconds: CycleMilliseconds

    public init(
        id: String,
        event: TimelineEventDefinition,
        cycleIndex: Int64,
        date: Date,
        absoluteMilliseconds: CycleMilliseconds
    ) {
        self.id = id
        self.event = event
        self.cycleIndex = cycleIndex
        self.date = date
        self.absoluteMilliseconds = absoluteMilliseconds
    }
}

public enum AlertEvaluationContext: Sendable {
    case foregroundTick
    case resume
    case clockCorrection
}

public struct AlertEvaluation: Equatable, Sendable {
    public let crossedEvents: [TimelineEventOccurrence]
    public let presentationEvents: [TimelineEventOccurrence]
    public let missedCount: Int
    public let needsStateReconciliation: Bool

    public init(
        crossedEvents: [TimelineEventOccurrence],
        presentationEvents: [TimelineEventOccurrence],
        missedCount: Int,
        needsStateReconciliation: Bool
    ) {
        self.crossedEvents = crossedEvents
        self.presentationEvents = presentationEvents
        self.missedCount = missedCount
        self.needsStateReconciliation = needsStateReconciliation
    }
}

public enum AlertPlanner {
    public static let foregroundFreshnessMilliseconds: CycleMilliseconds = 5_000

    /// Plans repeating alerts strictly after `now`, through the requested number
    /// of complete cycles. The anchor is the local fire-wave impact at cycle zero.
    public static func occurrences(
        anchor: Date,
        now: Date,
        horizonCycles: Int = 1,
        mutedCycle: Int64? = nil,
        cycle: RuptureCycle = .current,
        syncEpochID: UUID = UUID(uuidString: "00000000-0000-0000-0000-000000000000")!
    ) -> [AlertOccurrence] {
        guard horizonCycles > 0 else { return [] }

        let start = RuptureCycle.milliseconds(from: now.timeIntervalSince(anchor))
        let (horizonLength, multiplyOverflow) = cycle.cycleLengthMilliseconds
            .multipliedReportingOverflow(by: Int64(horizonCycles))
        precondition(!multiplyOverflow, "Alert horizon overflow")
        let (end, addOverflow) = start.addingReportingOverflow(horizonLength)
        precondition(!addOverflow, "Alert horizon overflow")

        return cycle.occurrences(after: start, through: end)
            .filter { $0.event.isAlert }
            .filter {
                !isMuted(
                    absoluteMilliseconds: $0.absoluteMilliseconds,
                    mutedCycle: mutedCycle,
                    cycleLength: cycle.cycleLengthMilliseconds
                )
            }
            .map { occurrence in
                AlertOccurrence(
                    id: identifier(
                        syncEpochID: syncEpochID,
                        definitionID: cycle.version.definitionID,
                        eventID: occurrence.event.id,
                        cycleIndex: occurrence.cycleIndex
                    ),
                    event: occurrence.event,
                    cycleIndex: occurrence.cycleIndex,
                    date: now.addingTimeInterval(occurrence.remaining),
                    absoluteMilliseconds: occurrence.absoluteMilliseconds
                )
            }
    }

    /// Evaluates `(previous, current]`. Resume and clock-correction contexts never
    /// replay historical threshold cues; callers render one current-state summary.
    public static func evaluateCrossing(
        from previousElapsed: TimeInterval,
        to currentElapsed: TimeInterval,
        context: AlertEvaluationContext,
        mutedCycle: Int64? = nil,
        freshnessMilliseconds: CycleMilliseconds = foregroundFreshnessMilliseconds,
        cycle: RuptureCycle = .current
    ) -> AlertEvaluation {
        evaluateCrossing(
            fromMilliseconds: RuptureCycle.milliseconds(from: previousElapsed),
            toMilliseconds: RuptureCycle.milliseconds(from: currentElapsed),
            context: context,
            mutedCycle: mutedCycle,
            freshnessMilliseconds: freshnessMilliseconds,
            cycle: cycle
        )
    }

    public static func evaluateCrossing(
        fromMilliseconds previousElapsed: CycleMilliseconds,
        toMilliseconds currentElapsed: CycleMilliseconds,
        context: AlertEvaluationContext,
        mutedCycle: Int64? = nil,
        freshnessMilliseconds: CycleMilliseconds = foregroundFreshnessMilliseconds,
        cycle: RuptureCycle = .current
    ) -> AlertEvaluation {
        guard currentElapsed >= previousElapsed else {
            return AlertEvaluation(
                crossedEvents: [],
                presentationEvents: [],
                missedCount: 0,
                needsStateReconciliation: true
            )
        }

        let crossed = cycle.occurrences(
            after: previousElapsed,
            through: currentElapsed
        ).filter { $0.event.isAlert }

        let eligible = crossed.filter {
            !isMuted(
                absoluteMilliseconds: $0.absoluteMilliseconds,
                mutedCycle: mutedCycle,
                cycleLength: cycle.cycleLengthMilliseconds
            )
        }

        switch context {
        case .resume, .clockCorrection:
            return AlertEvaluation(
                crossedEvents: crossed,
                presentationEvents: [],
                missedCount: eligible.count,
                needsStateReconciliation: true
            )

        case .foregroundTick:
            guard let latestTime = eligible.map(\.absoluteMilliseconds).max() else {
                return AlertEvaluation(
                    crossedEvents: crossed,
                    presentationEvents: [],
                    missedCount: 0,
                    needsStateReconciliation: false
                )
            }

            let lateness = currentElapsed - latestTime
            guard lateness <= freshnessMilliseconds else {
                return AlertEvaluation(
                    crossedEvents: crossed,
                    presentationEvents: [],
                    missedCount: eligible.count,
                    needsStateReconciliation: true
                )
            }

            let presentation = eligible.filter {
                $0.absoluteMilliseconds == latestTime
            }
            return AlertEvaluation(
                crossedEvents: crossed,
                presentationEvents: presentation,
                missedCount: eligible.count - presentation.count,
                needsStateReconciliation: false
            )
        }
    }

    public static func isMuted(
        absoluteMilliseconds: CycleMilliseconds,
        mutedCycle: Int64?,
        cycleLength: CycleMilliseconds = RuptureCycle.current.cycleLengthMilliseconds
    ) -> Bool {
        guard let mutedCycle else { return false }
        let (start, startOverflow) = mutedCycle.multipliedReportingOverflow(by: cycleLength)
        let (end, endOverflow) = start.addingReportingOverflow(cycleLength)
        precondition(!startOverflow && !endOverflow, "Muted cycle overflow")
        return start < absoluteMilliseconds && absoluteMilliseconds <= end
    }

    public static func identifier(
        syncEpochID: UUID,
        definitionID: String,
        eventID: String,
        cycleIndex: Int64
    ) -> String {
        "\(syncEpochID.uuidString)|\(definitionID)|\(eventID)|\(cycleIndex)"
    }
}
