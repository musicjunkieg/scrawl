import Foundation
@testable import ScrawlKit
import Testing

struct OnDeviceTranscriptTests {
  func word(_ t0: UInt64, _ t1: UInt64, _ text: String) -> OnDeviceTranscript.Word {
    .init(t0: t0, t1: t1, text: text)
  }

  func segments(_ json: [String: Any]) -> [String] {
    (json["segments"] as? [[String: Any]] ?? []).compactMap { $0["text"] as? String }
  }

  @Test func wordsSplitIntoSegmentsAtPausesAndSentenceEnds() {
    let json = OnDeviceTranscript.json(
      words: [
        word(1_000, 1_200, "this"), word(1_250, 1_500, "button."), word(1_600, 1_800, "And"),
        word(1_850, 2_000, "this"), word(4_000, 4_300, "here"), word(4_350, 4_500, " "),
      ],
      source: "apple"
    )
    #expect(json["source"] as? String == "apple")
    #expect(json["text"] as? String == "this button. And this here")
    #expect(segments(json) == ["this button.", "And this", "here"])
    #expect((json["words"] as? [[String: Any]])?.count == 5)
    let first = (json["segments"] as? [[String: Any]])?.first
    #expect(first?["t0"] as? UInt64 == 1_000 && first?["t1"] as? UInt64 == 1_500)
  }

  @Test func longRunsOfSpeechStillSplit() {
    let words = (0 ..< 40).map { word(UInt64($0) * 400, UInt64($0) * 400 + 300, "word") }
    #expect(segments(OnDeviceTranscript.json(words: words, source: "apple")).count == 2)
  }

  @Test func noTranscriptSaysWhy() {
    let json = OnDeviceTranscript.missing("The recording has no narration")
    #expect((json["missing"] as? [String: Any])?["reason"] as? String == "The recording has no narration")
  }
}

struct DeadlineTests {
  @Test func theFirstAnswerCounts() async {
    let answer: Int? = await Deadline.answer(within: .seconds(5)) { reply in
      reply(1)
      reply(2)
      return {}
    }
    #expect(answer == 1)
  }

  @Test func noAnswerInTimeStopsTheWork() async {
    final class Flag: @unchecked Sendable { var stopped = false }
    let flag = Flag()
    let answer: Int? = await Deadline.answer(within: .milliseconds(50)) { _ in { flag.stopped = true } }
    #expect(answer == nil)
    #expect(flag.stopped)
  }

  @Test func aLateAnswerIsIgnored() async {
    let answer: Int? = await Deadline.answer(within: .milliseconds(50)) { reply in
      Task {
        try? await Task.sleep(for: .milliseconds(300))
        reply(1)
      }
      return {}
    }
    #expect(answer == nil)
  }
}

#if SCRAWL_CORE
  struct OnDeviceTranscriptCompileTests {
    /// The transcript JSON is what the compiler reads: its words reach `summary.md`.
    @Test func theTranscriptReachesTheSummary() throws {
      let transcript = OnDeviceTranscript.json(
        words: [.init(t0: 200, t1: 400, text: "this"), .init(t0: 450, t1: 700, text: "button")],
        source: "apple"
      )
      let capture = try BundleCompiler.capture(
        timeline: Data(#"{"app":"UpDown"}"#.utf8), width: 40, height: 80, durationMs: 1000,
        fields: ["id": "spoken", "transcript": transcript]
      )
      let compiler = BundleCompiler()
      try compiler.addFrame(at: 0, width: 40, height: 80, rgba: Data(repeating: 90, count: 40 * 80 * 4))
      #expect(try compiler.compile(capture: capture).summary.contains("this button"))
    }
  }
#endif

#if SCRAWL_CORE && canImport(AVFoundation)
  import AVFoundation
  import CoreVideo

  struct OnDeviceBuildTests {
    /// A 2 s, 90×160 movie that changes colour each half second.
    func movie(at url: URL) async throws {
      let writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
      let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: 90, AVVideoHeightKey: 160,
      ])
      let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: 90, kCVPixelBufferHeightKey as String: 160,
      ])
      writer.add(input)
      #expect(writer.startWriting())
      writer.startSession(atSourceTime: .zero)
      for frame in 0 ..< 20 {
        while !input.isReadyForMoreMediaData { try await Task.sleep(for: .milliseconds(5)) }
        var buffer: CVPixelBuffer?
        CVPixelBufferCreate(nil, 90, 160, kCVPixelFormatType_32BGRA, nil, &buffer)
        let pixels = try #require(buffer)
        CVPixelBufferLockBaseAddress(pixels, [])
        memset(CVPixelBufferGetBaseAddress(pixels), Int32(40 + (frame / 5) * 50), CVPixelBufferGetDataSize(pixels))
        CVPixelBufferUnlockBaseAddress(pixels, [])
        adaptor.append(pixels, withPresentationTime: CMTime(value: CMTimeValue(frame), timescale: 10))
      }
      input.markAsFinished()
      await writer.finishWriting()
      #expect(writer.status == .completed)
    }

    @Test func aMovieAndItsTimelineBecomeABundle() async throws {
      let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      defer { try? FileManager.default.removeItem(at: folder) }
      let source = folder.appending(path: "20261023T010000.mp4")
      try await movie(at: source)
      let timeline = Data("""
      {"app":"UpDown","screen":{"width":90,"height":160,"scale":1},"markers":[1.2],
       "touches":[{"t":0.6,"id":1,"x":45,"y":80,"phase":"down","label":"Done"},
                  {"t":0.65,"id":1,"x":45,"y":80,"phase":"up"}],
       "events":[{"t":0.7,"label":"Done tapped"}]}
      """.utf8)
      let bundle = try await OnDeviceBuild.build(
        movie: source, timeline: timeline, id: "20261023T010000", createdAt: Date(),
        transcript: OnDeviceTranscript.missing("The recording has no narration"), into: folder
      )
      #expect(bundle.lastPathComponent == "20261023T010000.scrawl")
      for name in ["contact-sheet.png", "scrawl.json", "summary.md", "recording.mp4", "timeline.json"] {
        #expect(FileManager.default.fileExists(atPath: bundle.appending(path: name).path), "\(name)")
      }
      let frames = try FileManager.default.contentsOfDirectory(atPath: bundle.appending(path: "frames").path)
      #expect(!frames.isEmpty)
      let summary = try String(contentsOf: bundle.appending(path: "summary.md"), encoding: .utf8)
      #expect(summary.contains("Done tapped"))
      // It leaves the phone, so it's named by its folder, not by where it was built.
      #expect(summary.contains("· 20261023T010000.scrawl]"))
      #expect(!summary.contains(folder.path))
    }

    @Test func framesComeOutAtTheAskedSize() async throws {
      let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      defer { try? FileManager.default.removeItem(at: folder) }
      let source = folder.appending(path: "m.mp4")
      try await movie(at: source)
      let frames = try await OnDeviceBuild.frames(of: AVURLAsset(url: source), at: [0, 1500], edge: 80)
      #expect(frames.map(\.t) == [0, 1500])
      #expect(frames.allSatisfy { $0.height == 80 && $0.width == 45 && $0.rgba.count == 45 * 80 * 4 })
      // Different moments, different pictures.
      #expect(frames[0].rgba != frames[1].rgba)
    }
  }
#endif
