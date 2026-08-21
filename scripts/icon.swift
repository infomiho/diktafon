// Generates crates/diktafon/resources/diktafon.icns: the T3-landscape mark
// (a Rams T3 pocket radio lying flat - dial left, grille right, also drawn in
// statusbar.rs) on the Signal ground, in Apple's Big Sur icon grid (824pt
// body on a 1024pt canvas).
// Run from the repo root: swift scripts/icon.swift

import AppKit

let canvas: CGFloat = 1024
let body: CGFloat = 824
let cornerRadius: CGFloat = 185

// The mark's design box is 48x48; the device is a 40x22 rounded rect at
// (4,13), dial center (14.5,24) r6.6 hub r2.2, grille dots r1.7 on a 3x3
// grid at x 27/31.5/36, y 18.5/24/29.5.
let deviceScale: CGFloat = 14

func draw(size: CGFloat) -> NSImage {
    let scale = size / canvas
    let s = deviceScale * scale
    let image = NSImage(size: NSSize(width: size, height: size))
    image.lockFocus()

    let inset = (canvas - body) / 2 * scale
    let rect = NSRect(x: inset, y: inset, width: body * scale, height: body * scale)
    let squircle = NSBezierPath(roundedRect: rect, xRadius: cornerRadius * scale, yRadius: cornerRadius * scale)

    // theme::SURFACE_SUNKEN up to a step above theme::BACKGROUND.
    let top = NSColor(srgbRed: 0x1B / 255, green: 0x1E / 255, blue: 0x3F / 255, alpha: 1)
    let bottom = NSColor(srgbRed: 0x0B / 255, green: 0x0D / 255, blue: 0x20 / 255, alpha: 1)
    NSGradient(colors: [bottom, top])!.draw(in: squircle, angle: 90)

    // A faint aurora ember rising from the icon's foot.
    NSGraphicsContext.current?.saveGraphicsState()
    squircle.addClip()
    let ember = NSColor(srgbRed: 0xFF / 255, green: 0x5A / 255, blue: 0x36 / 255, alpha: 0.26)
    NSGradient(colorsAndLocations: (ember, 0), (ember.withAlphaComponent(0), 1))?
        .draw(fromCenter: NSPoint(x: size / 2, y: inset), radius: 0,
              toCenter: NSPoint(x: size / 2, y: inset), radius: 430 * scale,
              options: [])
    NSGraphicsContext.current?.restoreGraphicsState()

    // The device face, dial and grille punched out via even-odd.
    let deviceOrigin = NSPoint(x: size / 2 - 20 * s, y: size / 2 - 11 * s)
    func oval(_ path: NSBezierPath, _ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat) {
        // Design coordinates are top-left origin; AppKit's are bottom-left.
        let x = deviceOrigin.x + (cx - 4) * s
        let y = deviceOrigin.y + (22 - (cy - 13)) * s
        path.appendOval(in: NSRect(x: x - r * s, y: y - r * s, width: r * 2 * s, height: r * 2 * s))
    }
    let device = NSBezierPath(
        roundedRect: NSRect(x: deviceOrigin.x, y: deviceOrigin.y, width: 40 * s, height: 22 * s),
        xRadius: 6 * s, yRadius: 6 * s)
    device.windingRule = .evenOdd
    oval(device, 14.5, 24, 6.6)
    for gy in [18.5, 24, 29.5] {
        for gx in [27.0, 31.5, 36.0] {
            oval(device, gx, gy, 1.7)
        }
    }
    NSColor(srgbRed: 0xF1 / 255, green: 0xF2 / 255, blue: 0xFF / 255, alpha: 1).setFill()
    device.fill()

    // theme::ACCENT hub in the dial.
    let hub = NSBezierPath()
    hub.windingRule = .nonZero
    oval(hub, 14.5, 24, 2.2)
    NSColor(srgbRed: 0x98 / 255, green: 0x43 / 255, blue: 0xC0 / 255, alpha: 1).setFill()
    hub.fill()

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
