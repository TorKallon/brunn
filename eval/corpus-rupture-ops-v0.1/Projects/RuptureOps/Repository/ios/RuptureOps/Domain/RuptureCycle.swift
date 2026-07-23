import Foundation

public struct CycleCoordinate: Codable, Equatable, Sendable {
    public let cycleIndex: Int64
    public let offsetMilliseconds: CycleMilliseconds

    public init(cycleIndex: Int64, offsetMilliseconds: CycleMilliseconds) {
        self.cycleIndex = cycleIndex
        self.offsetMilliseconds = offsetMilliseconds
    }
}

public struct TimelineEventOccurrence: Codable, Equatable, Sendable, Identifiable {
    public let event: TimelineEventDefinition
    public let cycleIndex: Int64
    public let absoluteMilliseconds: CycleMilliseconds
    public let remainingMilliseconds: CycleMilliseconds

    public var id: String {
        "\(event.id)@\(cycleIndex)"
    }

    public var remaining: TimeInterval {
        TimeInterval(remainingMilliseconds) / 1_000
    }
}

public struct OpportunityState: Codable, Equatable, Sendable, Identifiable {
    public let definition: OpportunityDefinition
    public let remainingMilliseconds: CycleMilliseconds

    public var id: String { definition.id }
    public var title: String { definition.title }
    public var action: String { definition.action }
    public var caveat: String { definition.caveat }
    public var evidenceClass: EvidenceClass { definition.evidenceClass }
    public var confidence: OpportunityConfidence { definition.confidence }
    public var remaining: TimeInterval { TimeInterval(remainingMilliseconds) / 1_000 }
}

public struct CycleSnapshot: Codable, Equatable, Sendable {
    public let phase: MissionPhase
    public let sourcePhase: SourcePhase
    public let cycleIndex: Int64
    public let elapsedInCycleMilliseconds: CycleMilliseconds
    public let sourcePhaseRemainingMilliseconds: CycleMilliseconds
    public let missionPhaseRemainingMilliseconds: CycleMilliseconds
    public let heroRemainingMilliseconds: CycleMilliseconds
    public let title: String
    public let target: String
    public let threatInstruction: String
    public let opportunities: [OpportunityState]
    public let timelineEvents: [TimelineEventOccurrence]

    public var elapsedInCycle: TimeInterval {
        TimeInterval(elapsedInCycleMilliseconds) / 1_000
    }

    public var sourcePhaseRemaining: TimeInterval {
        TimeInterval(sourcePhaseRemainingMilliseconds) / 1_000
    }

    public var missionPhaseRemaining: TimeInterval {
        TimeInterval(missionPhaseRemainingMilliseconds) / 1_000
    }

    public var heroRemaining: TimeInterval {
        TimeInterval(heroRemainingMilliseconds) / 1_000
    }
}

public struct RuptureCycle: Equatable, Sendable {
    public let definition: RuptureCycleDefinition

    public init(definition: RuptureCycleDefinition) {
        self.definition = definition
    }

    public static let current: RuptureCycle = {
        let definition = RuptureCycleDefinition.currentR1
        do {
            try definition.validate()
        } catch {
            preconditionFailure("Invalid bundled Rupture cycle definition: \(error)")
        }
        return RuptureCycle(definition: definition)
    }()

    public var version: RuptureCycleVersion { definition.version }
    public var cycleLengthMilliseconds: CycleMilliseconds {
        definition.cycleLengthMilliseconds
    }

    public var cycleLength: TimeInterval {
        TimeInterval(cycleLengthMilliseconds) / 1_000
    }

    public func snapshot(elapsed: TimeInterval) -> CycleSnapshot {
        snapshot(elapsedMilliseconds: Self.milliseconds(from: elapsed))
    }

    public func snapshot(elapsedMilliseconds: CycleMilliseconds) -> CycleSnapshot {
        let coordinate = coordinate(elapsedMilliseconds: elapsedMilliseconds)
        let source = sourcePhase(at: coordinate.offsetMilliseconds)
        let mission = missionBand(at: coordinate.offsetMilliseconds)
        let hero = nextOccurrence(
            eventID: mission.heroEventID,
            after: elapsedMilliseconds
        )
        let opportunities = activeOpportunities(at: coordinate.offsetMilliseconds)
        let horizonEnd = adding(cycleLengthMilliseconds, to: elapsedMilliseconds)
        let timeline = occurrences(after: elapsedMilliseconds, through: horizonEnd)

        return CycleSnapshot(
            phase: mission.phase,
            sourcePhase: source.phase,
            cycleIndex: coordinate.cycleIndex,
            elapsedInCycleMilliseconds: coordinate.offsetMilliseconds,
            sourcePhaseRemainingMilliseconds:
                source.range.endMilliseconds - coordinate.offsetMilliseconds,
            missionPhaseRemainingMilliseconds:
                mission.range.endMilliseconds - coordinate.offsetMilliseconds,
            heroRemainingMilliseconds: hero.absoluteMilliseconds - elapsedMilliseconds,
            title: mission.title,
            target: hero.event.target,
            threatInstruction: mission.threatInstruction,
            opportunities: opportunities,
            timelineEvents: timeline
        )
    }

    public func coordinate(elapsed: TimeInterval) -> CycleCoordinate {
        coordinate(elapsedMilliseconds: Self.milliseconds(from: elapsed))
    }

    public func coordinate(elapsedMilliseconds: CycleMilliseconds) -> CycleCoordinate {
        let length = cycleLengthMilliseconds
        var cycleIndex = elapsedMilliseconds / length
        var offset = elapsedMilliseconds % length
        if offset < 0 {
            cycleIndex -= 1
            offset += length
        }
        return CycleCoordinate(cycleIndex: cycleIndex, offsetMilliseconds: offset)
    }

    public func occurrences(
        after startMilliseconds: CycleMilliseconds,
        through endMilliseconds: CycleMilliseconds
    ) -> [TimelineEventOccurrence] {
        guard endMilliseconds > startMilliseconds else { return [] }

        var result: [TimelineEventOccurrence] = []
        let length = cycleLengthMilliseconds

        for event in definition.events {
            let relativeStart = subtracting(event.offsetMilliseconds, from: startMilliseconds)
            var eventCycle = floorDivide(relativeStart, by: length) + 1
            var absolute = absoluteMilliseconds(
                cycleIndex: eventCycle,
                offsetMilliseconds: event.offsetMilliseconds
            )

            while absolute <= endMilliseconds {
                result.append(
                    TimelineEventOccurrence(
                        event: event,
                        cycleIndex: eventCycle,
                        absoluteMilliseconds: absolute,
                        remainingMilliseconds: absolute - startMilliseconds
                    )
                )
                eventCycle += 1
                absolute = adding(length, to: absolute)
            }
        }

        return result.sorted { lhs, rhs in
            if lhs.absoluteMilliseconds != rhs.absoluteMilliseconds {
                return lhs.absoluteMilliseconds < rhs.absoluteMilliseconds
            }
            if lhs.event.severity != rhs.event.severity {
                return lhs.event.severity > rhs.event.severity
            }
            return lhs.event.id < rhs.event.id
        }
    }

    public func nextOccurrence(
        eventID: String,
        after elapsedMilliseconds: CycleMilliseconds
    ) -> TimelineEventOccurrence {
        guard definition.events.contains(where: { $0.id == eventID }) else {
            preconditionFailure("Unknown timeline event: \(eventID)")
        }
        let end = adding(cycleLengthMilliseconds, to: elapsedMilliseconds)
        guard let occurrence = occurrences(after: elapsedMilliseconds, through: end)
            .first(where: { $0.event.id == eventID })
        else {
            preconditionFailure("No next occurrence for event: \(eventID)")
        }
        return occurrence
    }

    public static func milliseconds(from seconds: TimeInterval) -> CycleMilliseconds {
        precondition(seconds.isFinite, "Elapsed time must be finite")
        let scaled = seconds * 1_000
        precondition(
            scaled >= TimeInterval(Int64.min) && scaled <= TimeInterval(Int64.max),
            "Elapsed time exceeds supported range"
        )
        return CycleMilliseconds(scaled.rounded(.down))
    }

    private func sourcePhase(at offset: CycleMilliseconds) -> SourcePhaseDefinition {
        guard let phase = definition.sourcePhases.first(where: { $0.range.contains(offset) }) else {
            preconditionFailure("Cycle definition has no source phase at \(offset)")
        }
        return phase
    }

    private func missionBand(at offset: CycleMilliseconds) -> MissionBandDefinition {
        guard let band = definition.missionBands.first(where: { $0.range.contains(offset) }) else {
            preconditionFailure("Cycle definition has no mission band at \(offset)")
        }
        return band
    }

    private func activeOpportunities(at offset: CycleMilliseconds) -> [OpportunityState] {
        definition.opportunities
            .filter { $0.range.contains(offset) }
            .sorted {
                if $0.defaultRank != $1.defaultRank {
                    return $0.defaultRank < $1.defaultRank
                }
                return $0.id < $1.id
            }
            .map {
                OpportunityState(
                    definition: $0,
                    remainingMilliseconds: $0.range.endMilliseconds - offset
                )
            }
    }

    private func floorDivide(
        _ dividend: CycleMilliseconds,
        by divisor: CycleMilliseconds
    ) -> CycleMilliseconds {
        var quotient = dividend / divisor
        let remainder = dividend % divisor
        if remainder < 0 {
            quotient -= 1
        }
        return quotient
    }

    private func absoluteMilliseconds(
        cycleIndex: Int64,
        offsetMilliseconds: CycleMilliseconds
    ) -> CycleMilliseconds {
        let (base, multipliedOverflow) = cycleIndex.multipliedReportingOverflow(
            by: cycleLengthMilliseconds
        )
        let (absolute, addedOverflow) = base.addingReportingOverflow(offsetMilliseconds)
        precondition(!multipliedOverflow && !addedOverflow, "Cycle coordinate overflow")
        return absolute
    }

    private func adding(
        _ value: CycleMilliseconds,
        to base: CycleMilliseconds
    ) -> CycleMilliseconds {
        let (result, overflow) = base.addingReportingOverflow(value)
        precondition(!overflow, "Cycle time overflow")
        return result
    }

    private func subtracting(
        _ value: CycleMilliseconds,
        from base: CycleMilliseconds
    ) -> CycleMilliseconds {
        let (result, overflow) = base.subtractingReportingOverflow(value)
        precondition(!overflow, "Cycle time overflow")
        return result
    }
}
