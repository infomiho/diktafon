// Generates crates/diktafon/resources/diktafon.icns: the orbit mark (the
// pill's identity element, also drawn in statusbar.rs) on the Signal indigo
// ground, in Apple's Big Sur icon grid (824pt body on a 1024pt canvas).
// Run from the repo root: swift scripts/icon.swift

import AppKit

let canvas: CGFloat = 1024
let body: CGFloat = 824
let cornerRadius: CGFloat = 185

func draw(size: CGFloat) -> NSImage {
    let scale = size / canvas
    let image = NSImage(size: NSSize(width: size, height: size))
    image.lockFocus()

    let inset = (canvas - body) / 2 * scale
    let rect = NSRect(x: inset, y: inset, width: body * scale, height: body * scale)
    let path = NSBezierPath(roundedRect: rect, xRadius: cornerRadius * scale, yRadius: cornerRadius * scale)
    // theme::SURFACE
    NSColor(srgbRed: 0x14 / 255, green: 0x16 / 255, blue: 0x3A / 255, alpha: 1).setFill()
    path.fill()

    let center = NSPoint(x: size / 2, y: size / 2)
    func dot(_ point: NSPoint, _ radius: CGFloat, _ color: NSColor, glow: NSColor? = nil) {
        NSGraphicsContext.current?.saveGraphicsState()
        if let glow {
            let shadow = NSShadow()
            shadow.shadowColor = glow
            shadow.shadowBlurRadius = 60 * scale
            shadow.set()
        }
        color.setFill()
        NSBezierPath(ovalIn: NSRect(x: point.x - radius, y: point.y - radius, width: radius * 2, height: radius * 2)).fill()
        NSGraphicsContext.current?.restoreGraphicsState()
    }

    // theme::SIGNAL_RED center over a white satellite ring.
    let red = NSColor(srgbRed: 0xFF / 255, green: 0x3B / 255, blue: 0x4D / 255, alpha: 1)
    let ring: CGFloat = 190 * scale
    for i in 0..<8 {
        let angle = CGFloat(i) * .pi * 2 / 8
        let point = NSPoint(x: center.x + cos(angle) * ring, y: center.y + sin(angle) * ring)
        dot(point, 40 * scale, NSColor(white: 1, alpha: 0.92))
    }
    dot(center, 72 * scale, red, glow: red.withAlphaComponent(0.8))

    image.unlockFocus()
    return image
}

func writePNG(_ image: NSImage, to path: String) {
    guard let tiff = image.tiffRepresentation,
        let rep = NSBitmapImageRep(data: tiff),
        let png = rep.representation(using: .png, properties: [:])
    else { fatalError("rendering \(path)") }
    try! png.write(to: URL(fileURLWithPath: path))
}

let iconset = "diktafon.iconset"
try? FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    writePNG(draw(size: CGFloat(points)), to: "\(iconset)/icon_\(points)x\(points).png")
    writePNG(draw(size: CGFloat(points * 2)), to: "\(iconset)/icon_\(points)x\(points)@2x.png")
}

let task = Process()
task.launchPath = "/usr/bin/iconutil"
task.arguments = ["-c", "icns", iconset, "-o", "crates/diktafon/resources/diktafon.icns"]
task.launch()
task.waitUntilExit()
try? FileManager.default.removeItem(atPath: iconset)
print(task.terminationStatus == 0 ? "wrote crates/diktafon/resources/diktafon.icns" : "iconutil failed")
