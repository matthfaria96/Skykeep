// Skykeep's icon, drawn from scratch: a sky-blue squircle, a warm sun rising behind a glossy
// cloud, and a keyhole set into the cloud — your sky, kept.
//
// Usage: swift make-icon.swift <variant> <out.png> [size]
//   mac     the macOS icon: 824pt body on the 1024 grid, with Apple's drop shadow
//   flat    Windows and Linux: the tile filling the canvas, no shadow
//   tray    the tray: the cloud and keyhole alone, outlined so it reads on light and dark bars
//   notify  notifications: the cloud on a blue disc
import AppKit

let args = CommandLine.arguments
let variant = args[1]
let out = args[2]
let px = args.count > 3 ? Int(args[3])! : 1024
let s = CGFloat(px) / 1024

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
                           hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
let ctx = NSGraphicsContext(bitmapImageRep: rep)!
NSGraphicsContext.current = ctx
let cg = ctx.cgContext
cg.scaleBy(x: s, y: s)
let space = CGColorSpace(name: CGColorSpace.sRGB)!

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: a)
}
func gradient(_ colors: [CGColor], _ locs: [CGFloat]? = nil) -> CGGradient {
    CGGradient(colorsSpace: space, colors: colors as CFArray, locations: locs)!
}

/// Apple's continuous-corner tile, as a superellipse.
func squircle(_ r: CGRect) -> CGPath {
    let p = CGMutablePath()
    let n: CGFloat = 4.6
    let cx = r.midX, cy = r.midY, a = r.width / 2, b = r.height / 2
    for i in 0...720 {
        let t = CGFloat(i) / 720 * 2 * .pi
        let c = cos(t), sn = sin(t)
        let x = cx + a * (c < 0 ? -1 : 1) * pow(abs(c), 2 / n)
        let y = cy + b * (sn < 0 ? -1 : 1) * pow(abs(sn), 2 / n)
        i == 0 ? p.move(to: CGPoint(x: x, y: y)) : p.addLine(to: CGPoint(x: x, y: y))
    }
    p.closeSubpath()
    return p
}

/// The cloud on the 1024 grid: a pill-shaped base and three puffs, one shape.
func cloudPath() -> CGPath {
    let cloud = CGMutablePath()
    cloud.addPath(CGPath(roundedRect: CGRect(x: 214, y: 250, width: 596, height: 220), cornerWidth: 110, cornerHeight: 110, transform: nil))
    cloud.addEllipse(in: CGRect(x: 380 - 125, y: 455 - 125, width: 250, height: 250))
    cloud.addEllipse(in: CGRect(x: 528 - 168, y: 520 - 168, width: 336, height: 336))
    cloud.addEllipse(in: CGRect(x: 688 - 112, y: 455 - 112, width: 224, height: 224))
    return cloud
}

func keyholeParts() -> [CGPath] {
    let head = CGPath(ellipseIn: CGRect(x: 512 - 46, y: 398, width: 92, height: 92), transform: nil)
    let slot = CGMutablePath()
    slot.move(to: CGPoint(x: 512 - 22, y: 430))
    slot.addLine(to: CGPoint(x: 512 + 22, y: 430))
    slot.addLine(to: CGPoint(x: 512 + 34, y: 302))
    slot.addQuadCurve(to: CGPoint(x: 512 + 24, y: 292), control: CGPoint(x: 512 + 35, y: 292))
    slot.addLine(to: CGPoint(x: 512 - 24, y: 292))
    slot.addQuadCurve(to: CGPoint(x: 512 - 34, y: 302), control: CGPoint(x: 512 - 35, y: 292))
    slot.closeSubpath()
    return [head, slot]
}

func drawCloud(shadow: Bool) {
    let cloud = cloudPath()
    if shadow {
        cg.saveGState()
        cg.setShadow(offset: CGSize(width: 0, height: -22), blur: 46, color: rgb(0x0A2A80, 0.45))
        cg.addPath(cloud); cg.setFillColor(rgb(0xFFFFFF)); cg.fillPath()
        cg.restoreGState()
    }
    cg.saveGState()
    cg.addPath(cloud); cg.clip()
    cg.drawLinearGradient(gradient([rgb(0xFFFFFF), rgb(0xF3F7FF), rgb(0xD5E3FB)], [0, 0.5, 1]),
                          start: CGPoint(x: 512, y: 682), end: CGPoint(x: 512, y: 250), options: [])
    cg.restoreGState()
    for part in keyholeParts() {
        cg.saveGState()
        cg.addPath(part); cg.clip()
        cg.drawLinearGradient(gradient([rgb(0x1F55D6), rgb(0x3D86F5)]), start: CGPoint(x: 512, y: 490), end: CGPoint(x: 512, y: 292), options: [])
        cg.restoreGState()
    }
}

func drawTile(body: CGRect, shadow: Bool) {
    let shape = squircle(body)
    if shadow {
        cg.saveGState()
        cg.setShadow(offset: CGSize(width: 0, height: -14), blur: 30, color: rgb(0x000000, 0.32))
        cg.addPath(shape); cg.setFillColor(rgb(0x2A6EF0)); cg.fillPath()
        cg.restoreGState()
    }
    cg.saveGState()
    cg.addPath(shape); cg.clip()
    // Fit the 824-wide artwork to this tile.
    let k = body.width / 824
    cg.translateBy(x: body.midX, y: body.midY)
    cg.scaleBy(x: k, y: k)
    cg.translateBy(x: -512, y: -512)
    cg.drawLinearGradient(gradient([rgb(0x6CC6FF), rgb(0x2F86F6), rgb(0x1D4FD8)], [0, 0.55, 1]),
                          start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
    cg.drawRadialGradient(gradient([rgb(0xFFF2C4, 0.55), rgb(0xFFF2C4, 0)]),
                          startCenter: CGPoint(x: 690, y: 700), startRadius: 0, endCenter: CGPoint(x: 690, y: 700), endRadius: 420, options: [])
    let sun = CGPoint(x: 668, y: 612)
    cg.drawRadialGradient(gradient([rgb(0xFFD866, 0.65), rgb(0xFFD866, 0)]), startCenter: sun, startRadius: 120, endCenter: sun, endRadius: 250, options: [])
    cg.saveGState()
    cg.addEllipse(in: CGRect(x: sun.x - 132, y: sun.y - 132, width: 264, height: 264)); cg.clip()
    cg.drawLinearGradient(gradient([rgb(0xFFE27A), rgb(0xFFB531)]), start: CGPoint(x: sun.x, y: sun.y + 132), end: CGPoint(x: sun.x, y: sun.y - 132), options: [])
    cg.restoreGState()
    drawCloud(shadow: true)
    cg.drawLinearGradient(gradient([rgb(0xFFFFFF, 0.28), rgb(0xFFFFFF, 0)]), start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 640), options: [])
    cg.restoreGState()
    // Rim: light on top, darker at the bottom.
    cg.saveGState()
    cg.addPath(shape); cg.setLineWidth(4); cg.replacePathWithStrokedPath(); cg.clip()
    cg.drawLinearGradient(gradient([rgb(0xFFFFFF, 0.55), rgb(0xFFFFFF, 0.05), rgb(0x0A1F66, 0.35)], [0, 0.5, 1]),
                          start: CGPoint(x: body.midX, y: body.maxY), end: CGPoint(x: body.midX, y: body.minY), options: [])
    cg.restoreGState()
}

/// The cloud alone, enlarged and centred on the canvas.
func centredCloud(scale k: CGFloat, dy: CGFloat = 0) {
    cg.translateBy(x: 512, y: 512 + dy)
    cg.scaleBy(x: k, y: k)
    cg.translateBy(x: -512, y: -466)
}

switch variant {
case "mac":
    drawTile(body: CGRect(x: 100, y: 100, width: 824, height: 824), shadow: true)
case "flat":
    drawTile(body: CGRect(x: 24, y: 24, width: 976, height: 976), shadow: false)
case "tray":
    cg.saveGState()
    centredCloud(scale: 1.22)
    // A dark outline first, so the white cloud reads on a light bar too.
    cg.addPath(cloudPath()); cg.setLineWidth(46); cg.setStrokeColor(rgb(0x151A22)); cg.setLineJoin(.round); cg.strokePath()
    drawCloud(shadow: false)
    cg.restoreGState()
case "notify":
    cg.addEllipse(in: CGRect(x: 0, y: 0, width: 1024, height: 1024))
    cg.clip()
    cg.drawLinearGradient(gradient([rgb(0x6CC6FF), rgb(0x2F86F6), rgb(0x1D4FD8)], [0, 0.55, 1]),
                          start: CGPoint(x: 512, y: 1024), end: CGPoint(x: 512, y: 0), options: [])
    cg.saveGState()
    centredCloud(scale: 1.18, dy: -10)
    drawCloud(shadow: true)
    cg.restoreGState()
default:
    fatalError("unknown variant \(variant)")
}

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
