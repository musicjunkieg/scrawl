#if SCRAWL_CORE
import Foundation
import ScrawlCore

/// Scrawl's bundle compiler on the device (`crates/scrawl-ffi`, through ScrawlCore.xcframework):
/// add a recording's frames as RGBA pixels, optionally ask which keyframes it picks and add those
/// again at full size, then compile the capture (`scrawl.json`'s fields as JSON) into the
/// bundle's files. Not thread-safe; use one per recording.
public final class BundleCompiler {
  /// A message from the compiler: bad JSON, frames that don't match their size, no frames.
  public struct Failure: Error, CustomStringConvertible {
    public let description: String
  }

  /// A built bundle: its files in the order to save them (`summary.md` last) and `summary.md`'s
  /// text.
  public struct Bundle {
    public let files: [(name: String, data: Data)]
    public let summary: String

    /// Writes every file under `directory` (the `<id>.scrawl` folder), making folders as needed.
    public func write(to directory: URL) throws {
      let manager = FileManager.default
      for file in files {
        let url = directory.appendingPathComponent(file.name)
        try manager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try file.data.write(to: url, options: .atomic)
      }
    }
  }

  private let compiler: OpaquePointer

  public init() {
    compiler = scrawl_compiler_new()
  }

  deinit {
    scrawl_compiler_free(compiler)
  }

  /// Adds the screen as it looked `tMs` into the recording: `width`×`height` RGBA pixels. These
  /// can be small (the Mac app uses 480 px) when the keyframes are added again with
  /// `addFullFrame`. Throws when `rgba` isn't `width × height × 4` bytes.
  public func addFrame(at tMs: UInt64, width: Int, height: Int, rgba: Data) throws {
    try add(tMs, width, height, rgba, scrawl_compiler_add_frame)
  }

  /// Adds a keyframe at full size, at one of `keyframeTimes`.
  public func addFullFrame(at tMs: UInt64, width: Int, height: Int, rgba: Data) throws {
    try add(tMs, width, height, rgba, scrawl_compiler_add_full_frame)
  }

  private func add(
    _ tMs: UInt64, _ width: Int, _ height: Int, _ rgba: Data,
    _ call: (OpaquePointer?, UInt64, UInt32, UInt32, UnsafePointer<UInt8>?, Int) -> Bool
  ) throws {
    let (area, overflow) = width.multipliedReportingOverflow(by: height)
    guard let w = UInt32(exactly: width), let h = UInt32(exactly: height), w > 0, h > 0, !overflow,
          area <= Int.max / 4, rgba.count == area * 4
    else {
      throw Failure(description: "The frame at \(tMs) ms is \(rgba.count) bytes, not \(width)×\(height) RGBA")
    }
    let added = rgba.withUnsafeBytes { pixels in
      call(compiler, tMs, w, h, pixels.bindMemory(to: UInt8.self).baseAddress, pixels.count)
    }
    guard added else { throw Failure(description: "The compiler refused the frame at \(tMs) ms") }
  }

  /// The times (ms) of the keyframes `compile` will pick from the frames added so far.
  public func keyframeTimes(capture: String) throws -> [UInt64] {
    var count = 0
    var error: UnsafeMutablePointer<CChar>?
    let times = capture.withCString { scrawl_compiler_keyframe_times(compiler, $0, &count, &error) }
    guard let times else { throw Self.failure(error) }
    defer { scrawl_times_free(times, count) }
    return Array(UnsafeBufferPointer(start: times, count: count))
  }

  /// Builds the bundle from the frames added so far, which are used up.
  public func compile(capture: String) throws -> Bundle {
    var error: UnsafeMutablePointer<CChar>?
    let built = capture.withCString { scrawl_compiler_compile(compiler, $0, &error) }
    guard let built else { throw Self.failure(error) }
    defer { scrawl_bundle_free(built) }
    var files: [(name: String, data: Data)] = []
    for index in 0 ..< scrawl_bundle_file_count(built) {
      var length = 0
      guard let name = scrawl_bundle_file_name(built, index),
            let bytes = scrawl_bundle_file_data(built, index, &length)
      else { continue }
      files.append((String(cString: name), Data(bytes: bytes, count: length)))
    }
    let summary = scrawl_bundle_summary(built).map { String(cString: $0) } ?? ""
    return Bundle(files: files, summary: summary)
  }

  /// The capture JSON for a ScrawlKit recording, built from its timeline the way the Mac app
  /// imports one (touches become taps and swipes, ink moves to the bundle's coordinates).
  /// `width`×`height` is the movie's size in pixels; `fields` replace the timeline's capture
  /// fields (`id`, `createdAtMs`, `transcript`, `video`, `audio`, …).
  public static func capture(
    timeline: Data, width: Int, height: Int, durationMs: UInt64, fields: [String: Any] = [:]
  ) throws -> String {
    guard let w = UInt32(exactly: width), let h = UInt32(exactly: height) else {
      throw Failure(description: "The movie is \(width)×\(height), not a valid size")
    }
    let fieldsJSON = try String(decoding: JSONSerialization.data(withJSONObject: fields), as: UTF8.self)
    var error: UnsafeMutablePointer<CChar>?
    let json = String(decoding: timeline, as: UTF8.self).withCString { timeline in
      fieldsJSON.withCString { scrawl_capture_from_timeline(timeline, w, h, durationMs, $0, &error) }
    }
    guard let json else { throw failure(error) }
    defer { scrawl_string_free(json) }
    return String(cString: json)
  }

  private static func failure(_ error: UnsafeMutablePointer<CChar>?) -> Failure {
    defer { scrawl_string_free(error) }
    return Failure(description: error.map { String(cString: $0) } ?? "The compiler failed")
  }
}
#endif
