#if SCRAWL_CORE
import AVFoundation
import CoreGraphics
import Foundation
import Speech

/// Builds a recording's `.scrawl` bundle on the phone, the way the Mac builds one it receives
/// (`build_movie_bundle`): candidate frames decoded small for keyframe selection, the chosen
/// keyframes decoded again at full size, the narration transcribed, then the compiler
/// (`BundleCompiler`) writes the images, `scrawl.json` and `summary.md`. The movie and timeline
/// are copied in beside them.
enum OnDeviceBuild {
  /// The Mac's sizes for a phone movie (`src/phone.rs`): candidates fit 320 px, keyframes 1600 px.
  static let candidateEdge = 320
  static let keyframeEdge = 1600
  /// At most this many candidates, at least 250 ms apart.
  static let maxCandidates: UInt64 = 480

  /// Builds `<id>.scrawl` inside `directory` from `movie` and its `timeline` (the JSON ScrawlKit
  /// wrote), with `transcript` (`OnDeviceTranscript`) as what was said. Returns the bundle's
  /// folder.
  static func build(
    movie: URL, timeline: Data, id: String, createdAt: Date, transcript: [String: Any], into directory: URL
  ) async throws -> URL {
    let asset = AVURLAsset(url: movie)
    let durationMs = try await UInt64(max(asset.load(.duration).seconds, 0) * 1000)
    guard durationMs > 0, let track = try await asset.loadTracks(withMediaType: .video).first else {
      throw BundleCompiler.Failure(description: "The recording is empty")
    }
    let natural = try await track.load(.naturalSize).applying(track.load(.preferredTransform))
    let (width, height) = (Int(abs(natural.width).rounded()), Int(abs(natural.height).rounded()))

    let bundle = directory.appending(path: "\(id).scrawl", directoryHint: .isDirectory)
    let videoName = "recording.\(movie.pathExtension.isEmpty ? "mp4" : movie.pathExtension.lowercased())"
    let capture = try BundleCompiler.capture(
      timeline: timeline, width: width, height: height, durationMs: durationMs,
      fields: [
        "id": id,
        "directory": bundle.path,
        "createdAtMs": UInt64(createdAt.timeIntervalSince1970 * 1000),
        "transcript": transcript,
        "video": videoName,
      ]
    )

    let compiler = BundleCompiler()
    let every = max(durationMs / maxCandidates, 250)
    let candidates = Array(stride(from: UInt64(0), to: durationMs, by: Int(every)))
    // Each frame goes straight to the compiler (which copies it), so Swift never holds a pass.
    try await forEachFrame(of: asset, at: candidates, edge: candidateEdge) { frame in
      try compiler.addFrame(at: frame.t, width: frame.width, height: frame.height, rgba: frame.rgba)
    }
    let keyframes = try compiler.keyframeTimes(capture: capture)
    try await forEachFrame(of: asset, at: keyframes, edge: keyframeEdge) { frame in
      try compiler.addFullFrame(at: frame.t, width: frame.width, height: frame.height, rgba: frame.rgba)
    }
    let built = try compiler.compile(capture: capture)

    let manager = FileManager.default
    try? manager.removeItem(at: bundle)
    try built.write(to: bundle)
    try manager.copyItem(at: movie, to: bundle.appending(path: videoName))
    try timeline.write(to: bundle.appending(path: "timeline.json"))
    return bundle
  }

  struct Frame {
    let t: UInt64
    let width: Int
    let height: Int
    let rgba: Data
  }

  /// The movie at `times` (ms), each fit to `edge` on its long side, as RGBA pixels. A time the
  /// movie can't give is skipped.
  static func frames(of asset: AVAsset, at times: [UInt64], edge: Int) async throws -> [Frame] {
    var frames: [Frame] = []
    try await forEachFrame(of: asset, at: times, edge: edge) { frames.append($0) }
    return frames
  }

  /// `frames(of:at:edge:)`, one at a time: `body` gets each frame as it's decoded.
  static func forEachFrame(
    of asset: AVAsset, at times: [UInt64], edge: Int, _ body: (Frame) throws -> Void
  ) async throws {
    let generator = AVAssetImageGenerator(asset: asset)
    generator.appliesPreferredTrackTransform = true
    generator.maximumSize = CGSize(width: edge, height: edge)
    generator.requestedTimeToleranceBefore = .zero
    generator.requestedTimeToleranceAfter = .zero
    for t in times {
      try Task.checkCancellation()
      guard let decoded = try? await generator.image(at: CMTime(value: CMTimeValue(t), timescale: 1000)),
            let frame = rgba(decoded.image, at: t)
      else { continue }
      try body(frame)
    }
  }

  private static func rgba(_ image: CGImage, at t: UInt64) -> Frame? {
    let (width, height) = (image.width, image.height)
    var pixels = Data(count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let context = CGContext(
        data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
      ) else { return false }
      context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
      return true
    }
    return drawn ? Frame(t: t, width: width, height: height, rgba: pixels) : nil
  }
}

/// The narration of a recording, transcribed on the phone with Apple's speech recognition
/// (offline: `requiresOnDeviceRecognition`).
enum OnDeviceSpeech {
  /// What `movie` says, as `OnDeviceTranscript` JSON. Without narration, without the app's
  /// `NSSpeechRecognitionUsageDescription`, or when the person declines, the transcript is empty
  /// and says why. The first call asks the person for permission.
  static func transcript(of movie: URL, narrated: Bool) async -> [String: Any] {
    guard narrated else { return OnDeviceTranscript.missing("The recording has no narration") }
    // Asking without the usage string would end the app.
    guard Bundle.main.object(forInfoDictionaryKey: "NSSpeechRecognitionUsageDescription") != nil else {
      print("Scrawl: add NSSpeechRecognitionUsageDescription to the app's Info.plist to transcribe on the phone")
      return OnDeviceTranscript.missing("The app doesn't ask for speech recognition")
    }
    guard await authorized() else {
      return OnDeviceTranscript.missing("Speech recognition isn't allowed for this app")
    }
    guard let recognizer = SFSpeechRecognizer() ?? SFSpeechRecognizer(locale: Locale(identifier: "en-US")),
          recognizer.supportsOnDeviceRecognition
    else {
      return OnDeviceTranscript.missing("This phone can't recognize speech offline in its language")
    }
    let audio = FileManager.default.temporaryDirectory.appending(path: "scrawl-\(UUID().uuidString).m4a")
    defer { try? FileManager.default.removeItem(at: audio) }
    guard await exportAudio(of: movie, to: audio) else {
      return OnDeviceTranscript.missing("The narration couldn't be read from the recording")
    }
    let request = SFSpeechURLRecognitionRequest(url: audio)
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = false
    request.addsPunctuation = true
    // Offline recognition runs at a few times real time; past this it isn't coming.
    let seconds = (try? await AVURLAsset(url: movie).load(.duration).seconds) ?? 0
    let limit = Duration.seconds(60 + 2 * max(seconds.isFinite ? seconds : 0, 0))
    guard let transcription = await recognize(request, with: recognizer, within: limit) else {
      return OnDeviceTranscript.missing("Speech recognition didn't finish")
    }
    let words = transcription.segments.map {
      OnDeviceTranscript.Word(
        t0: UInt64(max($0.timestamp, 0) * 1000),
        t1: UInt64(max($0.timestamp + $0.duration, 0) * 1000),
        text: $0.substring
      )
    }
    return OnDeviceTranscript.json(words: words, source: "apple")
  }

  private static func authorized() async -> Bool {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized: return true
    case .notDetermined:
      let status = await withCheckedContinuation { continuation in
        SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
      }
      return status == .authorized
    default: return false
    }
  }

  /// The movie's sound alone, which speech recognition reads more reliably than a movie.
  private static func exportAudio(of movie: URL, to audio: URL) async -> Bool {
    guard let session = AVAssetExportSession(asset: AVURLAsset(url: movie), presetName: AVAssetExportPresetAppleM4A)
    else { return false }
    do {
      try await session.export(to: audio, as: .m4a)
      return true
    } catch {
      print("Scrawl: couldn't read the narration: \(error.localizedDescription)")
      return false
    }
  }

  /// The final transcription, or nil on an error or when `limit` passes first.
  private static func recognize(
    _ request: SFSpeechURLRecognitionRequest, with recognizer: SFSpeechRecognizer, within limit: Duration
  ) async -> SFTranscription? {
    final class Once: @unchecked Sendable {
      private let lock = NSLock()
      private var done = false
      func claim() -> Bool { lock.withLock { defer { done = true }; return !done } }
    }
    let once = Once()
    return await withCheckedContinuation { continuation in
      let task = recognizer.recognitionTask(with: request) { result, error in
        if let result, result.isFinal {
          if once.claim() { continuation.resume(returning: result.bestTranscription) }
        } else if error != nil {
          if once.claim() { continuation.resume(returning: nil) }
        }
      }
      // The recognizer has no deadline of its own, and a build must not wait forever on it.
      Task {
        try? await Task.sleep(for: limit)
        if once.claim() {
          task.cancel()
          continuation.resume(returning: nil)
        }
      }
    }
  }
}

#endif
