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
  /// `addFullFrame`.
  public func addFrame(at tMs: UInt64, width: Int, height: Int, rgba: Data) {
    rgba.withUnsafeBytes { pixels in
      _ = scrawl_compiler_add_frame(
        compiler, tMs, UInt32(width), UInt32(height),
        pixels.bindMemory(to: UInt8.self).baseAddress, pixels.count
      )
    }
  }

  /// Adds a keyframe at full size, at one of `keyframeTimes`.
  public func addFullFrame(at tMs: UInt64, width: Int, height: Int, rgba: Data) {
    rgba.withUnsafeBytes { pixels in
      _ = scrawl_compiler_add_full_frame(
        compiler, tMs, UInt32(width), UInt32(height),
        pixels.bindMemory(to: UInt8.self).baseAddress, pixels.count
      )
    }
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

  private static func failure(_ error: UnsafeMutablePointer<CChar>?) -> Failure {
    defer { scrawl_string_free(error) }
    return Failure(description: error.map { String(cString: $0) } ?? "The compiler failed")
  }
}
#endif
