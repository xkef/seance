#!/usr/bin/env swift

import CoreGraphics
import Foundation

func intValue(_ value: Any?) -> Int? {
    switch value {
    case let number as NSNumber:
        return number.intValue
    case let int as Int:
        return int
    case let double as Double:
        return Int(double)
    default:
        return nil
    }
}

func doubleValue(_ value: Any?) -> Double? {
    switch value {
    case let number as NSNumber:
        return number.doubleValue
    case let int as Int:
        return Double(int)
    case let double as Double:
        return double
    default:
        return nil
    }
}

guard CommandLine.arguments.count == 2, let pid = Int(CommandLine.arguments[1]) else {
    fputs("usage: window_info.swift <pid>\n", stderr)
    exit(2)
}

guard let rawList = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
    fputs("failed to query window list\n", stderr)
    exit(1)
}

var best: [String: Any]?
var bestArea = -1.0

for info in rawList {
    guard intValue(info[kCGWindowOwnerPID as String]) == pid else { continue }
    guard intValue(info[kCGWindowLayer as String]) == 0 else { continue }
    guard let bounds = info[kCGWindowBounds as String] as? [String: Any] else { continue }
    guard
        let x = doubleValue(bounds["X"]),
        let y = doubleValue(bounds["Y"]),
        let width = doubleValue(bounds["Width"]),
        let height = doubleValue(bounds["Height"])
    else {
        continue
    }
    guard width > 100, height > 100 else { continue }

    let area = width * height
    guard area >= bestArea else { continue }
    bestArea = area
    best = [
        "window_id": intValue(info[kCGWindowNumber as String]) ?? 0,
        "owner_name": info[kCGWindowOwnerName as String] as? String ?? "",
        "title": info[kCGWindowName as String] as? String ?? "",
        "x": Int(x.rounded()),
        "y": Int(y.rounded()),
        "width": Int(width.rounded()),
        "height": Int(height.rounded()),
    ]
}

guard let payload = best else {
    exit(1)
}

let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([0x0a]))
