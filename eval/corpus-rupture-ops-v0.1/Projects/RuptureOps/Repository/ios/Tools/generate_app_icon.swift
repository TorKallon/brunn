#!/usr/bin/env swift

import AppKit
import Foundation

let outputPath = CommandLine.arguments.dropFirst().first
    ?? "ios/RuptureOps/Resources/Assets.xcassets/AppIcon.appiconset/AppIcon.png"
let size = NSSize(width: 1024, height: 1024)
let image = NSImage(size: size)

image.lockFocus()

NSColor(calibratedRed: 0.015, green: 0.018, blue: 0.025, alpha: 1).setFill()
NSBezierPath(rect: NSRect(origin: .zero, size: size)).fill()

let railX: CGFloat = 512
let railWidth: CGFloat = 18
let topY: CGFloat = 790
let middleY: CGFloat = 505
let bottomY: CGFloat = 220

func fillRoundedRect(_ rect: NSRect, radius: CGFloat, color: NSColor) {
    color.setFill()
    NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
}

func drawNode(center: NSPoint, radius: CGFloat, color: NSColor, filled: Bool) {
    let rect = NSRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2)
    let path = NSBezierPath(ovalIn: rect)
    path.lineWidth = 18
    color.setStroke()
    path.stroke()
    if filled {
        let inset = rect.insetBy(dx: 22, dy: 22)
        color.setFill()
        NSBezierPath(ovalIn: inset).fill()
    }
}

let indigo = NSColor(calibratedRed: 0.565, green: 0.620, blue: 1.000, alpha: 1)
let amber = NSColor(calibratedRed: 1.000, green: 0.690, blue: 0.220, alpha: 1)
let red = NSColor(calibratedRed: 1.000, green: 0.250, blue: 0.290, alpha: 1)

fillRoundedRect(NSRect(x: railX - railWidth / 2, y: middleY, width: railWidth, height: topY - middleY), radius: 9, color: indigo)
fillRoundedRect(NSRect(x: railX - railWidth / 2, y: bottomY, width: railWidth, height: middleY - bottomY), radius: 9, color: amber)
fillRoundedRect(NSRect(x: railX - railWidth / 2, y: 145, width: railWidth, height: bottomY - 145), radius: 9, color: red)

drawNode(center: NSPoint(x: railX, y: topY), radius: 68, color: indigo, filled: true)
drawNode(center: NSPoint(x: railX, y: middleY), radius: 68, color: amber, filled: true)
drawNode(center: NSPoint(x: railX, y: bottomY), radius: 72, color: red, filled: false)

red.setStroke()
let slashPath = NSBezierPath()
slashPath.lineWidth = 20
slashPath.lineCapStyle = .round
for offset in stride(from: -42 as CGFloat, through: 42, by: 28) {
    slashPath.move(to: NSPoint(x: railX - 42 + offset, y: bottomY - 44))
    slashPath.line(to: NSPoint(x: railX + 42 + offset, y: bottomY + 44))
}
NSGraphicsContext.current?.saveGraphicsState()
NSBezierPath(ovalIn: NSRect(x: railX - 54, y: bottomY - 54, width: 108, height: 108)).addClip()
slashPath.stroke()
NSGraphicsContext.current?.restoreGraphicsState()

image.unlockFocus()

guard
    let tiff = image.tiffRepresentation,
    let bitmap = NSBitmapImageRep(data: tiff),
    let png = bitmap.representation(using: .png, properties: [:])
else {
    fatalError("Could not render app icon")
}

let outputURL = URL(fileURLWithPath: outputPath)
try FileManager.default.createDirectory(at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)
try png.write(to: outputURL, options: .atomic)
print("Generated RuptureOps app icon at \(outputURL.path)")
