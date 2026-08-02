import AppKit

let size = NSSize(width: 1024, height: 1024)
let image = NSImage(size: size)
image.lockFocus()

let background = NSBezierPath(roundedRect: NSRect(origin: .zero, size: size), xRadius: 190, yRadius: 190)
NSColor(calibratedRed: 0.055, green: 0.384, blue: 0.286, alpha: 1).setFill()
background.fill()

let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center
let attributes: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 560, weight: .bold),
    .foregroundColor: NSColor.white,
    .paragraphStyle: paragraph,
]
let letter = NSAttributedString(string: "S", attributes: attributes)
letter.draw(in: NSRect(x: 0, y: 190, width: 1024, height: 650))

image.unlockFocus()

guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: 1024,
    pixelsHigh: 1024,
    bitsPerSample: 8,
    samplesPerPixel: 3,
    hasAlpha: false,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fatalError("Could not render the Straylight app icon")
}
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
image.draw(in: NSRect(origin: .zero, size: size))
NSGraphicsContext.restoreGraphicsState()

guard let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("Could not encode the Straylight app icon")
}

let output = URL(fileURLWithPath: CommandLine.arguments[1])
try png.write(to: output, options: .atomic)
