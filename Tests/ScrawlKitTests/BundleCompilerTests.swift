#if SCRAWL_CORE
import Foundation
import ScrawlKit
import Testing

struct BundleCompilerTests {
  let capture = """
  {"id":"swift-test","createdAtMs":1700000000000,"source":"ios",
   "target":{"mode":"window","app":"Example","displayId":0,
             "frame":{"x":0,"y":0,"width":64,"height":40,"scale":1}},
   "durationMs":2000,"events":[{"kind":"click","t":900,"x":0.5,"y":0.5}],
   "transcript":{"source":"test","text":"this button",
                 "segments":[{"t0":800,"t1":1500,"text":"this button"}]}}
  """

  func pixels(_ width: Int, _ height: Int, _ shade: UInt8) -> Data {
    Data(repeating: shade, count: width * height * 4)
  }

  /// A PNG's width, from its IHDR chunk.
  func width(_ png: Data) -> UInt32 {
    png.dropFirst(16).prefix(4).reduce(0) { $0 << 8 | UInt32($1) }
  }

  @Test func smallCandidatesPickTheKeyframesAndFullFramesDrawThem() throws {
    let compiler = BundleCompiler()
    for (i, t) in [0, 900, 1800].enumerated() {
      compiler.addFrame(at: UInt64(t), width: 64, height: 40, rgba: pixels(64, 40, UInt8(30 + i * 60)))
    }
    let times = try compiler.keyframeTimes(capture: capture)
    #expect(!times.isEmpty)
    for t in times {
      compiler.addFullFrame(at: t, width: 128, height: 80, rgba: pixels(128, 80, UInt8(truncatingIfNeeded: t / 10)))
    }
    let bundle = try compiler.compile(capture: capture)
    #expect(bundle.files.first?.name == "contact-sheet.png")
    #expect(bundle.files.last?.name == "summary.md")
    let frames = bundle.files.filter { $0.name.hasPrefix("frames/") }
    #expect(frames.count == times.count)
    for frame in frames {
      #expect(width(frame.data) == 128, "\(frame.name) is drawn from its full frame")
    }
    #expect(bundle.summary.contains("this button"))

    let directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("\(UUID().uuidString)/swift-test.scrawl")
    defer { try? FileManager.default.removeItem(at: directory.deletingLastPathComponent()) }
    try bundle.write(to: directory)
    for file in bundle.files {
      #expect(FileManager.default.fileExists(atPath: directory.appendingPathComponent(file.name).path))
    }
  }

  @Test func badInputThrowsInsteadOfCrashing() {
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().compile(capture: "{}")
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().compile(capture: capture)
    }
  }
}
#endif
