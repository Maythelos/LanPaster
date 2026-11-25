import AppKit
import Foundation

// macOS 图标规范：1024 画布内只放 824×824 的主体，四周留 100px 透明安全区。
// 满画布图标在 Finder 列表的 16pt 尺寸下会被系统套浅灰底板并缩小。
let canvasSize: CGFloat = 1024
let contentInset: CGFloat = 100
let contentSize = canvasSize - contentInset * 2
let iconNames = [
    "icon_16x16.png",
    "icon_16x16@2x.png",
    "icon_32x32.png",
    "icon_32x32@2x.png",
    "icon_128x128.png",
    "icon_128x128@2x.png",
    "icon_256x256.png",
    "icon_256x256@2x.png",
    "icon_512x512.png",
    "icon_512x512@2x.png",
]

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("用法: swift build-macos-icon.swift <源 PNG> <输出 icns>\n".data(using: .utf8)!)
    exit(1)
}

let sourceURL = URL(fileURLWithPath: arguments[1])
let outputURL = URL(fileURLWithPath: arguments[2])

guard let sourceImage = NSImage(contentsOf: sourceURL) else {
    FileHandle.standardError.write("无法读取图标源文件: \(sourceURL.path)\n".data(using: .utf8)!)
    exit(1)
}

let paddedImage = NSImage(size: NSSize(width: canvasSize, height: canvasSize))
paddedImage.lockFocus()
NSGraphicsContext.current?.imageInterpolation = .high
sourceImage.draw(
    in: NSRect(x: contentInset, y: contentInset, width: contentSize, height: contentSize),
    from: .zero,
    operation: .sourceOver,
    fraction: 1
)
paddedImage.unlockFocus()

let fileManager = FileManager.default
let iconsetURL = fileManager.temporaryDirectory
    .appendingPathComponent("LanPaster-\(UUID().uuidString).iconset")

do {
    try fileManager.createDirectory(at: iconsetURL, withIntermediateDirectories: true)
    for name in iconNames {
        let pixelSize = Int(name
            .replacingOccurrences(of: "icon_", with: "")
            .components(separatedBy: "x")[0])!
        let scale = name.contains("@2x") ? 2 : 1
        let pixels = pixelSize * scale

        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: pixels,
            pixelsHigh: pixels,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ) else {
            throw NSError(domain: "LanPasterIcon", code: 1, userInfo: [NSLocalizedDescriptionKey: "无法创建 \(name)"])
        }

        rep.size = NSSize(width: pixels, height: pixels)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSGraphicsContext.current?.imageInterpolation = .high
        paddedImage.draw(
            in: NSRect(x: 0, y: 0, width: pixels, height: pixels),
            from: .zero,
            operation: .sourceOver,
            fraction: 1
        )
        NSGraphicsContext.restoreGraphicsState()

        guard let png = rep.representation(using: .png, properties: [:]) else {
            throw NSError(domain: "LanPasterIcon", code: 2, userInfo: [NSLocalizedDescriptionKey: "无法编码 \(name)"])
        }
        try png.write(to: iconsetURL.appendingPathComponent(name))
    }

    try fileManager.createDirectory(
        at: outputURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )

    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
    process.arguments = ["-c", "icns", iconsetURL.path, "-o", outputURL.path]
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw NSError(domain: "LanPasterIcon", code: 3, userInfo: [NSLocalizedDescriptionKey: "iconutil 执行失败"])
    }
} catch {
    try? fileManager.removeItem(at: iconsetURL)
    FileHandle.standardError.write("生成 macOS 图标失败: \(error.localizedDescription)\n".data(using: .utf8)!)
    exit(1)
}

try? fileManager.removeItem(at: iconsetURL)
print("macOS 图标已生成: \(outputURL.path)")
