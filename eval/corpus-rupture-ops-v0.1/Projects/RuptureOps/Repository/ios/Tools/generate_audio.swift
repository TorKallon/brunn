#!/usr/bin/env swift

import Foundation

struct Tone {
    let frequency: Double
    let duration: Double
    let gap: Double
    let amplitude: Double
}

private let sampleRate = 44_100
private let channels: UInt16 = 1
private let bitsPerSample: UInt16 = 16

private extension Data {
    mutating func appendLittleEndian<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }
}

private func pcmSamples(for tones: [Tone]) -> [Int16] {
    var samples: [Int16] = []

    for tone in tones {
        let toneSampleCount = Int(tone.duration * Double(sampleRate))
        let gapSampleCount = Int(tone.gap * Double(sampleRate))
        let rampDuration = min(0.025, tone.duration / 4)

        for index in 0..<toneSampleCount {
            let time = Double(index) / Double(sampleRate)
            let remaining = tone.duration - time
            let envelope = min(1, min(time / rampDuration, remaining / rampDuration))
            let wave = sin(2 * .pi * tone.frequency * time)
            let value = max(-1, min(1, wave * envelope * tone.amplitude))
            samples.append(Int16(value * Double(Int16.max)))
        }

        samples.append(contentsOf: repeatElement(0, count: gapSampleCount))
    }

    return samples
}

private func writeWAV(tones: [Tone], to url: URL) throws {
    let samples = pcmSamples(for: tones)
    let bytesPerSample = Int(bitsPerSample / 8)
    let dataByteCount = UInt32(samples.count * bytesPerSample)
    let byteRate = UInt32(sampleRate) * UInt32(channels) * UInt32(bytesPerSample)
    let blockAlign = channels * UInt16(bytesPerSample)

    var data = Data()
    data.append(contentsOf: "RIFF".utf8)
    data.appendLittleEndian(UInt32(36) + dataByteCount)
    data.append(contentsOf: "WAVE".utf8)
    data.append(contentsOf: "fmt ".utf8)
    data.appendLittleEndian(UInt32(16))
    data.appendLittleEndian(UInt16(1))
    data.appendLittleEndian(channels)
    data.appendLittleEndian(UInt32(sampleRate))
    data.appendLittleEndian(byteRate)
    data.appendLittleEndian(blockAlign)
    data.appendLittleEndian(bitsPerSample)
    data.append(contentsOf: "data".utf8)
    data.appendLittleEndian(dataByteCount)

    for sample in samples {
        data.appendLittleEndian(sample)
    }

    try data.write(to: url, options: .atomic)
}

let outputDirectory = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? "ios/RuptureOps/Resources/Sounds")
try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

let cues: [(String, [Tone])] = [
    (
        "hostiles-warning.wav",
        [
            Tone(frequency: 420, duration: 0.18, gap: 0.13, amplitude: 0.26),
            Tone(frequency: 420, duration: 0.18, gap: 0.02, amplitude: 0.26)
        ]
    ),
    (
        "hostiles-return.wav",
        [
            Tone(frequency: 420, duration: 0.18, gap: 0.10, amplitude: 0.30),
            Tone(frequency: 660, duration: 0.18, gap: 0.10, amplitude: 0.28),
            Tone(frequency: 420, duration: 0.28, gap: 0.02, amplitude: 0.32)
        ]
    ),
    (
        "ruptura-warning.wav",
        [
            Tone(frequency: 520, duration: 0.16, gap: 0.08, amplitude: 0.24),
            Tone(frequency: 680, duration: 0.16, gap: 0.08, amplitude: 0.26),
            Tone(frequency: 840, duration: 0.24, gap: 0.02, amplitude: 0.28)
        ]
    ),
    (
        "ruptura-impact.wav",
        [
            Tone(frequency: 880, duration: 0.12, gap: 0.06, amplitude: 0.30),
            Tone(frequency: 540, duration: 0.12, gap: 0.06, amplitude: 0.30),
            Tone(frequency: 880, duration: 0.12, gap: 0.06, amplitude: 0.30),
            Tone(frequency: 540, duration: 0.30, gap: 0.02, amplitude: 0.34)
        ]
    )
]

for (name, tones) in cues {
    try writeWAV(tones: tones, to: outputDirectory.appendingPathComponent(name))
}

print("Generated \(cues.count) original RuptureOps alert sounds in \(outputDirectory.path)")

