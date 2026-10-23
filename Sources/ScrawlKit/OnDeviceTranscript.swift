import Foundation

/// The transcript of a recording compiled on the phone, in `scrawl.json`'s shape
/// (`TranscriptData`): every word with its time, and the words grouped into segments the way the
/// summary quotes them. Plain values, so the grouping is testable without Speech.
enum OnDeviceTranscript {
  /// A recognized word, in milliseconds from the start of the recording.
  struct Word: Equatable {
    var t0: UInt64
    var t1: UInt64
    var text: String
  }

  /// A pause this long ends a segment.
  static let segmentGap: UInt64 = 700
  /// A segment this long ends at its next word.
  static let segmentLimit: UInt64 = 12_000

  /// The transcript of `words`, recognized by `source` (`"apple"`). Segments end at a pause, a
  /// sentence's end, or once they run long.
  static func json(words: [Word], source: String) -> [String: Any] {
    let words = words
      .map { Word(t0: $0.t0, t1: max($0.t1, $0.t0), text: $0.text.trimmingCharacters(in: .whitespacesAndNewlines)) }
      .filter { !$0.text.isEmpty }
      .sorted { $0.t0 < $1.t0 }
    var segments: [[String: Any]] = []
    var current: [Word] = []
    func close() {
      guard let first = current.first, let last = current.last else { return }
      segments.append(["t0": first.t0, "t1": last.t1, "text": current.map(\.text).joined(separator: " ")])
      current = []
    }
    for word in words {
      if let last = current.last, let first = current.first,
         word.t0 > last.t1 + segmentGap || last.text.last.map(".?!".contains) == true
           || word.t0 - first.t0 >= segmentLimit
      {
        close()
      }
      current.append(word)
    }
    close()
    return [
      "source": source,
      "text": words.map(\.text).joined(separator: " "),
      "segments": segments,
      "words": words.map { ["t0": $0.t0, "t1": $0.t1, "text": $0.text] },
    ]
  }

  /// No transcript, and why (shown in the summary): no narration, speech not allowed, …
  static func missing(_ reason: String) -> [String: Any] {
    ["source": "none", "text": "", "segments": [], "missing": ["reason": reason]]
  }
}
