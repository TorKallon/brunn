import AppKit
import Foundation

// Usage: swift generate_app_icon.swift [--mark PATH] [--mono PATH]
//   [--glyph PATH] [--waterline PATH] [--hero PATH] [--ios-only]

let scriptURL = URL(fileURLWithPath: #filePath)
let iosRoot = scriptURL.deletingLastPathComponent().deletingLastPathComponent()
let repositoryRoot = iosRoot.deletingLastPathComponent().deletingLastPathComponent()

let arguments = Array(CommandLine.arguments.dropFirst())

func sourceURL(for option: String, defaultPath: String) -> URL {
    guard let index = arguments.firstIndex(of: option) else {
        return repositoryRoot.appendingPathComponent(defaultPath)
    }
    guard arguments.indices.contains(index + 1) else {
        fatalError("\(option) requires a path")
    }
    return URL(fileURLWithPath: arguments[index + 1])
}

let markSource = sourceURL(
    for: "--mark",
    defaultPath: "assets/brand/brunn-well-1024.png"
)
let monoSource = sourceURL(
    for: "--mono",
    defaultPath: "assets/brand/brunn-well-mono.svg"
)
let glyphSource = sourceURL(
    for: "--glyph",
    defaultPath: "assets/brand/brunn-well-glyph.svg"
)
let waterlineSource = sourceURL(
    for: "--waterline",
    defaultPath: "assets/brand/brunn-waterline-1024.png"
)
let heroSource = sourceURL(
    for: "--hero",
    defaultPath: "assets/brand/brunn-hero-wide.png"
)
let iosOnly = arguments.contains("--ios-only")

let appIconDirectory = iosRoot.appendingPathComponent(
    "Brunn/Resources/Assets.xcassets/AppIcon.appiconset"
)
let launchDirectory = iosRoot.appendingPathComponent(
    "Brunn/Resources/Assets.xcassets/LaunchWaterline.imageset"
)
let webDirectory = repositoryRoot.appendingPathComponent("apps/web/public")

func requireSquareMaster(_ source: URL, name: String) throws -> Data {
    let data = try Data(contentsOf: source)
    guard let bitmap = NSBitmapImageRep(data: data) else {
        fatalError("The \(name) master is not a readable bitmap")
    }
    guard bitmap.pixelsWide == 1024, bitmap.pixelsHigh == 1024 else {
        fatalError("The \(name) master must be exactly 1024 × 1024 pixels")
    }
    guard bitmap.representation(using: .png, properties: [:]) != nil, !bitmap.hasAlpha else {
        fatalError("The \(name) master must be an opaque PNG")
    }
    return data
}

func requireWideMaster(_ source: URL, name: String) throws {
    let data = try Data(contentsOf: source)
    guard let bitmap = NSBitmapImageRep(data: data) else {
        fatalError("The \(name) master is not a readable bitmap")
    }
    guard bitmap.pixelsWide == 3840, bitmap.pixelsHigh == 2160 else {
        fatalError("The \(name) master must be exactly 3840 × 2160 pixels")
    }
    guard bitmap.representation(using: .png, properties: [:]) != nil, !bitmap.hasAlpha else {
        fatalError("The \(name) master must be an opaque PNG")
    }
}

func rasterizedPNG(from source: URL, size: Int, hasAlpha: Bool) throws -> Data {
    guard let image = NSImage(contentsOf: source) else {
        fatalError("Cannot render \(source.path)")
    }
    guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else {
        fatalError("Cannot create the sRGB color space")
    }
    let alphaInfo: CGImageAlphaInfo = hasAlpha ? .premultipliedLast : .noneSkipLast
    guard let bitmapContext = CGContext(
        data: nil,
        width: size,
        height: size,
        bitsPerComponent: 8,
        bytesPerRow: size * 4,
        space: colorSpace,
        bitmapInfo: alphaInfo.rawValue
    ) else {
        fatalError("Cannot allocate a \(size) × \(size) bitmap")
    }

    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    let context = NSGraphicsContext(cgContext: bitmapContext, flipped: false)
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    context.shouldAntialias = true
    if hasAlpha {
        bitmapContext.clear(CGRect(x: 0, y: 0, width: size, height: size))
    }
    image.draw(
        in: NSRect(x: 0, y: 0, width: size, height: size),
        from: NSRect(origin: .zero, size: image.size),
        operation: .copy,
        fraction: 1,
        respectFlipped: true,
        hints: [.interpolation: NSImageInterpolation.high]
    )
    guard let renderedImage = bitmapContext.makeImage() else {
        fatalError("Cannot finish rendering \(source.lastPathComponent)")
    }
    let bitmap = NSBitmapImageRep(cgImage: renderedImage)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        fatalError("Cannot encode \(source.lastPathComponent) as PNG")
    }
    return data
}

func write(_ data: Data, to destination: URL) throws {
    try FileManager.default.createDirectory(
        at: destination.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try data.write(to: destination, options: .atomic)
    print("Wrote \(destination.path)")
}

func writeLosslessWebP(from png: Data, to destination: URL) throws {
    let temporary = FileManager.default.temporaryDirectory
        .appendingPathComponent("brunn-well-\(UUID().uuidString).png")
    try png.write(to: temporary, options: .atomic)
    defer { try? FileManager.default.removeItem(at: temporary) }

    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = [
        "cwebp", "-quiet", "-lossless", "-exact",
        temporary.path, "-o", destination.path,
    ]
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        fatalError("cwebp failed while writing \(destination.path)")
    }
    print("Wrote \(destination.path)")
}

func openGraphPNG(from source: URL) throws -> Data {
    let width = 1200
    let height = 630
    guard let image = NSImage(contentsOf: source) else {
        fatalError("Cannot render \(source.path)")
    }
    guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else {
        fatalError("Cannot create the sRGB color space")
    }
    guard let bitmapContext = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: width * 4,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
    ) else {
        fatalError("Cannot allocate the Open Graph bitmap")
    }

    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    let context = NSGraphicsContext(cgContext: bitmapContext, flipped: false)
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    context.shouldAntialias = true

    let sourceSize = CGSize(width: 3840, height: 2160)
    let scale = max(CGFloat(width) / sourceSize.width, CGFloat(height) / sourceSize.height)
    let destinationSize = CGSize(width: sourceSize.width * scale, height: sourceSize.height * scale)
    let destination = CGRect(
        x: (CGFloat(width) - destinationSize.width) / 2,
        y: (CGFloat(height) - destinationSize.height) / 2,
        width: destinationSize.width,
        height: destinationSize.height
    )
    image.draw(
        in: destination,
        from: NSRect(origin: .zero, size: image.size),
        operation: .copy,
        fraction: 1,
        respectFlipped: true,
        hints: [.interpolation: NSImageInterpolation.high]
    )

    guard let wordmarkFont = NSFont(name: "Georgia", size: 92) else {
        fatalError("Georgia is required to typeset the Brunn wordmark")
    }
    let wordmark = NSAttributedString(
        string: "brunn",
        attributes: [
            .font: wordmarkFont,
            .foregroundColor: NSColor(srgbRed: 245 / 255, green: 248 / 255, blue: 255 / 255, alpha: 1),
            .kern: -2.76,
        ]
    )
    wordmark.draw(at: NSPoint(x: 90, y: 334))

    let tagline = NSAttributedString(
        string: "The well your agents draw from.",
        attributes: [
            .font: NSFont.systemFont(ofSize: 28, weight: .regular),
            .foregroundColor: NSColor(srgbRed: 189 / 255, green: 201 / 255, blue: 226 / 255, alpha: 1),
        ]
    )
    tagline.draw(at: NSPoint(x: 100, y: 284))

    guard let renderedImage = bitmapContext.makeImage() else {
        fatalError("Cannot finish the Open Graph bitmap")
    }
    let bitmap = NSBitmapImageRep(cgImage: renderedImage)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        fatalError("Cannot encode the Open Graph bitmap as PNG")
    }
    return data
}

let markPNG = try requireSquareMaster(markSource, name: "Still Water mark")
_ = try requireSquareMaster(waterlineSource, name: "Still Water waterline")

try write(markPNG, to: appIconDirectory.appendingPathComponent("AppIcon.png"))
try write(
    rasterizedPNG(from: monoSource, size: 1024, hasAlpha: true),
    to: appIconDirectory.appendingPathComponent("AppIcon-tinted.png")
)

for (size, filename) in [
    (240, "LaunchWaterline.png"),
    (480, "LaunchWaterline@2x.png"),
    (720, "LaunchWaterline@3x.png"),
] {
    try write(
        rasterizedPNG(from: waterlineSource, size: size, hasAlpha: false),
        to: launchDirectory.appendingPathComponent(filename)
    )
}

if !iosOnly {
    try requireWideMaster(heroSource, name: "Still Water hero")
    try write(
        Data(contentsOf: glyphSource),
        to: webDirectory.appendingPathComponent("favicon.svg")
    )
    try write(
        rasterizedPNG(from: glyphSource, size: 32, hasAlpha: false),
        to: webDirectory.appendingPathComponent("favicon-32.png")
    )
    try write(
        rasterizedPNG(from: glyphSource, size: 16, hasAlpha: false),
        to: webDirectory.appendingPathComponent("favicon-16.png")
    )
    try write(
        rasterizedPNG(from: markSource, size: 180, hasAlpha: false),
        to: webDirectory.appendingPathComponent("apple-touch-icon.png")
    )
    try writeLosslessWebP(
        from: rasterizedPNG(from: markSource, size: 128, hasAlpha: false),
        to: webDirectory.appendingPathComponent("brunn-well-128.webp")
    )
    try write(
        openGraphPNG(from: heroSource),
        to: webDirectory.appendingPathComponent("og.png")
    )
}
