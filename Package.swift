// swift-tools-version: 6.2
import Foundation
import PackageDescription

// ScrawlCore.xcframework is Scrawl's bundle compiler (crates/scrawl-ffi). With it, ScrawlKit's
// `BundleCompiler` builds bundles on the device. A copy built here by scripts/build-xcframework.sh
// (target/) comes first; otherwise the release's, from GitHub Releases (`coreRelease`, filled in
// by docs/releasing.md). SCRAWL_CORE=1 or 0 decides when set; without ScrawlCore, ScrawlKit builds
// as before and `BundleCompiler` is absent. Xcode opened from the Finder sees no environment.
let coreRelease: (url: String, checksum: String)? = nil
let localCore = FileManager.default.fileExists(atPath: Context.packageDirectory + "/target/ScrawlCore.xcframework")
let core = Context.environment["SCRAWL_CORE"].map { $0 == "1" } ?? (localCore || coreRelease != nil)
let coreTarget: Target = if !localCore, let coreRelease {
  .binaryTarget(name: "ScrawlCore", url: coreRelease.url, checksum: coreRelease.checksum)
} else {
  .binaryTarget(name: "ScrawlCore", path: "target/ScrawlCore.xcframework")
}
// `#if SCRAWL_CORE` in the sources and tests, so a test can't be compiled out silently.
let swiftSettings: [SwiftSetting] = [.swiftLanguageMode(.v5)] + (core ? [.define("SCRAWL_CORE")] : [])

let package = Package(
  name: "ScrawlKit",
  // macOS only so `swift test` can run the platform-independent parts (the outbox, the bundle
  // compiler) on a Mac; everything UIKit is `#if os(iOS)`.
  platforms: [.iOS(.v18), .macOS(.v15)],
  products: [
    .library(name: "ScrawlKit", targets: ["ScrawlKit"]),
  ],
  targets: [
    .target(
      name: "ScrawlKit",
      dependencies: core ? ["ScrawlCore"] : [],
      path: "Sources/ScrawlKit",
      swiftSettings: swiftSettings,
      // What the compiler's Mac slice links (frame text in Core Text); iOS needs only libSystem.
      linkerSettings: core ? [
        .linkedFramework("CoreText", .when(platforms: [.macOS])),
        .linkedFramework("CoreGraphics", .when(platforms: [.macOS])),
        .linkedFramework("CoreFoundation", .when(platforms: [.macOS])),
        .linkedLibrary("iconv", .when(platforms: [.macOS])),
      ] : []
    ),
    .testTarget(
      name: "ScrawlKitTests",
      dependencies: ["ScrawlKit"],
      path: "Tests/ScrawlKitTests",
      swiftSettings: swiftSettings
    ),
  ] + (core ? [coreTarget] : [])
)
