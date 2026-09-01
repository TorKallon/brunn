import AppKit
import Foundation

let scriptURL = URL(fileURLWithPath: #filePath)
let iosRoot = scriptURL.deletingLastPathComponent().deletingLastPathComponent()
let repositoryRoot = iosRoot.deletingLastPathComponent().deletingLastPathComponent()

let defaultSource = repositoryRoot
    .appendingPathComponent("assets/brand/brunn-night-signal-1024.png")
let defaultOutput = iosRoot
    .appendingPathComponent("Brunn/Resources/Assets.xcassets/AppIcon.appiconset/AppIcon.png")

let source = CommandLine.arguments.count > 1
    ? URL(fileURLWithPath: CommandLine.arguments[1])
    : defaultSource
let output = CommandLine.arguments.count > 2
    ? URL(fileURLWithPath: CommandLine.arguments[2])
    : defaultOutput

let png = try Data(contentsOf: source)
guard let bitmap = NSBitmapImageRep(data: png) else {
    fatalError("The Night Signal master is not a readable bitmap")
}
guard bitmap.pixelsWide == 1024, bitmap.pixelsHigh == 1024 else {
    fatalError("The Night Signal master must be exactly 1024 × 1024 pixels")
}
guard bitmap.representation(using: .png, properties: [:]) != nil, !bitmap.hasAlpha else {
    fatalError("The Night Signal master must be an opaque PNG")
}

try png.write(to: output, options: .atomic)
print("Installed Night Signal app icon at \(output.path)")
