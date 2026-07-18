// Compleo OCR - Swift Vision Framework bridge with CJK support
// Returns text with position info for speaker identification
import Foundation
import Vision
import AppKit

guard CommandLine.arguments.count > 1 else {
    let error: [String: Any] = ["error": "Usage: compleo-ocr <image-path>"]
    printJSON(error)
    exit(1)
}

let imagePath = CommandLine.arguments[1]
let url = URL(fileURLWithPath: imagePath)

guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
      let cgImage = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    let error: [String: Any] = ["error": "Failed to load image at \(imagePath)"]
    printJSON(error)
    exit(1)
}

let imageWidth = CGFloat(cgImage.width)
let imageHeight = CGFloat(cgImage.height)

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en", "ja", "ko"]
if #available(macOS 13.0, *) {
    request.revision = VNRecognizeTextRequestRevision3
}

let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])

do {
    try handler.perform([request])
} catch {
    let err: [String: Any] = ["error": "Vision request failed: \(error.localizedDescription)"]
    printJSON(err)
    exit(1)
}

guard let results = request.results, !results.isEmpty else {
    let output: [String: Any] = ["text": "", "count": 0]
    printJSON(output)
    exit(0)
}

// Build structured output with position info
// Vision bounding box: origin at bottom-left, normalized 0-1
// We convert to: x_center as percentage of image width
// If x_center > 0.55 → right side (user's message)
// If x_center < 0.45 → left side (other's message)

// Filter out obvious UI noise
let uiNoisePatterns = [
    "Please enter a message",
    "Send",
    "New Line",
    "Watermark",
    "Group Check",
    "All Read",
    "Unread",
    "群聊成员",
    "群主",
    "管理员",
]

var lines: [String] = []

// Sort by Y position (top to bottom = higher Y first in Vision coords, but we want top first)
let sorted = results.sorted { a, b in
    a.boundingBox.origin.y > b.boundingBox.origin.y
}

for observation in sorted {
    guard let candidate = observation.topCandidates(1).first else { continue }
    let text = candidate.string.trimmingCharacters(in: .whitespaces)
    
    // Skip very short text (likely icons/symbols)
    if text.count < 2 { continue }
    
    // Skip UI noise
    let isNoise = uiNoisePatterns.contains { noise in
        text.contains(noise)
    }
    if isNoise { continue }
    
    let bbox = observation.boundingBox
    let xCenter = bbox.origin.x + bbox.size.width / 2.0

    // Determine side
    let side: String
    if xCenter > 0.6 {
        side = "→"  // Right side = user
    } else if xCenter < 0.4 {
        side = "←"  // Left side = other person
    } else {
        side = "·"  // Center = system message / timestamp
    }

    lines.append("\(side) \(text)")
}

let formattedText = lines.joined(separator: "\n")

let output: [String: Any] = [
    "text": formattedText,
    "count": results.count,
]
printJSON(output)

func printJSON(_ dict: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: dict, options: []),
       let str = String(data: data, encoding: .utf8)
    {
        print(str)
    }
}
