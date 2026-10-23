// swift-tools-version: 6.2
import Foundation
import PackageDescription

// ScrawlCore.xcframework is Scrawl's bundle compiler (crates/scrawl-ffi), built by
// scripts/build-xcframework.sh into target/. With it, ScrawlKit's `BundleCompiler` builds bundles
// on the device. Until a release publishes it, it's linked once it has been built (or with
// SCRAWL_CORE=1; SCRAWL_CORE=0 leaves it out); without it ScrawlKit builds as before and
// `BundleCompiler` is absent. Xcode opened from the Finder sees no environment, hence the file.
let core = Context.environment["SCRAWL_CORE"].map { $0 == "1" }
  ?? FileManager.default.fileExists(atPath: Context.packageDirectory + "/target/ScrawlCore.xcframework")
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
  ] + (core ? [.binaryTarget(name: "ScrawlCore", path: "target/ScrawlCore.xcframework")] : [])
)
