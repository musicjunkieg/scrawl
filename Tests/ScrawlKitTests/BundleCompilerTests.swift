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

  /// A PNG's width and height, from its IHDR chunk.
  func size(_ png: Data) -> (width: UInt32, height: UInt32) {
    let read = { (at: Int) -> UInt32 in png.dropFirst(at).prefix(4).reduce(0) { $0 << 8 | UInt32($1) } }
    return (read(16), read(20))
  }

  @Test func smallCandidatesPickTheKeyframesAndFullFramesDrawThem() throws {
    let compiler = BundleCompiler()
    for (i, t) in [0, 900, 1800].enumerated() {
      try compiler.addFrame(at: UInt64(t), width: 64, height: 40, rgba: pixels(64, 40, UInt8(30 + i * 60)))
    }
    let times = try compiler.keyframeTimes(capture: capture)
    #expect(!times.isEmpty)
    for t in times {
      try compiler.addFullFrame(at: t, width: 128, height: 80, rgba: pixels(128, 80, UInt8(truncatingIfNeeded: t / 10)))
    }
    let bundle = try compiler.compile(capture: capture)
    #expect(bundle.files.first?.name == "contact-sheet.png")
    #expect(bundle.files.last?.name == "summary.md")
    let frames = bundle.files.filter { $0.name.hasPrefix("frames/") }
    #expect(frames.count == times.count)
    for frame in frames {
      let (width, height) = size(frame.data)
      #expect(width == 128 && height == 80, "\(frame.name) is drawn from its full frame")
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

  @Test func aTimelineBecomesACaptureThatCompiles() throws {
    let timeline = Data("""
    {"app":"UpDown","screen":{"width":400,"height":800,"scale":3},"markers":[0.5],
     "touches":[{"t":0.2,"id":1,"x":200,"y":400,"phase":"down"},
                {"t":0.25,"id":1,"x":200,"y":400,"phase":"up"}]}
    """.utf8)
    let capture = try BundleCompiler.capture(
      timeline: timeline, width: 40, height: 80, durationMs: 1000,
      fields: ["id": "kit-swift", "transcript": ["source": "apple", "text": "right here", "segments": []]]
    )
    let compiler = BundleCompiler()
    try compiler.addFrame(at: 0, width: 40, height: 80, rgba: pixels(40, 80, 90))
    let bundle = try compiler.compile(capture: capture)
    #expect(bundle.summary.contains("kit-swift.scrawl"))
    #expect(bundle.summary.contains("right here"))
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler.capture(timeline: Data("[".utf8), width: 40, height: 80, durationMs: 1000)
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler.capture(timeline: timeline, width: 40, height: 80, durationMs: 1000, fields: ["createdAtMs": Date()])
    }
  }

  @Test func badInputThrowsInsteadOfCrashing() {
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().addFrame(at: 0, width: -1, height: 40, rgba: pixels(64, 40, 0))
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().addFrame(at: 0, width: 64, height: 40, rgba: Data())
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().addFrame(at: 0, width: 64, height: 40, rgba: pixels(64, 39, 0))
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().compile(capture: "{}")
    }
    #expect(throws: BundleCompiler.Failure.self) {
      try BundleCompiler().compile(capture: capture)
    }
  }
}
#endif
