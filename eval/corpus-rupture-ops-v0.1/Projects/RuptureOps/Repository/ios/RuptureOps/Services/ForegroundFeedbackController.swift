import AVFAudio
import CoreHaptics
import Foundation

enum FeedbackCue: String, Codable, Sendable {
    case phaseChange
    case hostilesWarning
    case hostilesReturn
    case rupturaWarning
    case rupturaImpact

    var soundResource: String? {
        switch self {
        case .phaseChange:
            nil
        case .hostilesWarning:
            "hostiles-warning"
        case .hostilesReturn:
            "hostiles-return"
        case .rupturaWarning:
            "ruptura-warning"
        case .rupturaImpact:
            "ruptura-impact"
        }
    }
}

@MainActor
protocol ForegroundFeedbackPlaying: AnyObject {
    func playIfNeeded(
        occurrenceID: String,
        cue: FeedbackCue,
        soundEnabled: Bool,
        hapticsEnabled: Bool,
        audibleInSilentMode: Bool
    )
    func preview(cue: FeedbackCue, soundEnabled: Bool, hapticsEnabled: Bool, audibleInSilentMode: Bool)
    func resetDeduplication()
}

@MainActor
final class ForegroundFeedbackController: ForegroundFeedbackPlaying {
    static let shared = ForegroundFeedbackController()

    private var audioPlayer: AVAudioPlayer?
    private var hapticEngine: CHHapticEngine?
    private var playedOccurrenceIDs = Set<String>()
    private var playedOrder: [String] = []

    private init() {
        prepareHaptics()
    }

    func playIfNeeded(
        occurrenceID: String,
        cue: FeedbackCue,
        soundEnabled: Bool,
        hapticsEnabled: Bool,
        audibleInSilentMode: Bool
    ) {
        guard playedOccurrenceIDs.insert(occurrenceID).inserted else { return }
        playedOrder.append(occurrenceID)
        trimDeduplicationHistory()
        play(cue: cue, soundEnabled: soundEnabled, hapticsEnabled: hapticsEnabled, audibleInSilentMode: audibleInSilentMode)
    }

    func preview(cue: FeedbackCue, soundEnabled: Bool, hapticsEnabled: Bool, audibleInSilentMode: Bool) {
        play(cue: cue, soundEnabled: soundEnabled, hapticsEnabled: hapticsEnabled, audibleInSilentMode: audibleInSilentMode)
    }

    func resetDeduplication() {
        playedOccurrenceIDs.removeAll()
        playedOrder.removeAll()
    }

    private func play(cue: FeedbackCue, soundEnabled: Bool, hapticsEnabled: Bool, audibleInSilentMode: Bool) {
        if soundEnabled {
            playSound(cue: cue, audibleInSilentMode: audibleInSilentMode)
        }
        if hapticsEnabled {
            playHaptic(cue: cue)
        }
    }

    private func playSound(cue: FeedbackCue, audibleInSilentMode: Bool) {
        guard
            let resource = cue.soundResource,
            let url = Bundle.main.url(forResource: resource, withExtension: "wav")
        else { return }

        do {
            let session = AVAudioSession.sharedInstance()
            if audibleInSilentMode {
                try session.setCategory(.playback, mode: .default, options: [.duckOthers])
            } else {
                try session.setCategory(.ambient, mode: .default, options: [.mixWithOthers])
            }
            try session.setActive(true)

            let player = try AVAudioPlayer(contentsOf: url)
            player.prepareToPlay()
            player.play()
            audioPlayer = player
        } catch {
            assertionFailure("Unable to play RuptureOps alert sound: \(error)")
        }
    }

    private func prepareHaptics() {
        guard CHHapticEngine.capabilitiesForHardware().supportsHaptics else { return }

        do {
            let engine = try CHHapticEngine()
            engine.isAutoShutdownEnabled = true
            engine.resetHandler = { [weak self] in
                Task { @MainActor [weak self] in
                    try? self?.hapticEngine?.start()
                }
            }
            try engine.start()
            hapticEngine = engine
        } catch {
            hapticEngine = nil
        }
    }

    private func playHaptic(cue: FeedbackCue) {
        guard let hapticEngine else { return }

        let events: [CHHapticEvent]
        switch cue {
        case .phaseChange:
            events = [transient(at: 0, intensity: 0.35, sharpness: 0.35)]
        case .hostilesWarning, .rupturaWarning:
            events = [
                transient(at: 0, intensity: 0.65, sharpness: 0.55),
                transient(at: 0.22, intensity: 0.65, sharpness: 0.55)
            ]
        case .hostilesReturn, .rupturaImpact:
            events = [
                transient(at: 0, intensity: 1, sharpness: 0.72),
                transient(at: 0.20, intensity: 1, sharpness: 0.72),
                transient(at: 0.40, intensity: 1, sharpness: 0.72)
            ]
        }

        do {
            try hapticEngine.start()
            let pattern = try CHHapticPattern(events: events, parameters: [])
            try hapticEngine.makePlayer(with: pattern).start(atTime: 0)
        } catch {
            prepareHaptics()
        }
    }

    private func transient(at time: TimeInterval, intensity: Float, sharpness: Float) -> CHHapticEvent {
        CHHapticEvent(
            eventType: .hapticTransient,
            parameters: [
                CHHapticEventParameter(parameterID: .hapticIntensity, value: intensity),
                CHHapticEventParameter(parameterID: .hapticSharpness, value: sharpness)
            ],
            relativeTime: time
        )
    }

    private func trimDeduplicationHistory() {
        let retainedCount = 128
        guard playedOrder.count > retainedCount else { return }
        let removalCount = playedOrder.count - retainedCount
        let removed = Array(playedOrder.prefix(removalCount))
        playedOrder.removeFirst(removalCount)
        playedOccurrenceIDs.subtract(removed)
    }
}

@MainActor
final class NoopFeedbackController: ForegroundFeedbackPlaying {
    func playIfNeeded(
        occurrenceID: String,
        cue: FeedbackCue,
        soundEnabled: Bool,
        hapticsEnabled: Bool,
        audibleInSilentMode: Bool
    ) {}

    func preview(cue: FeedbackCue, soundEnabled: Bool, hapticsEnabled: Bool, audibleInSilentMode: Bool) {}
    func resetDeduplication() {}
}
