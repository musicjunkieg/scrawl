// swift-tools-version: 6.0
// A small Mac app to try ScrawlKit for macOS, building bundles on this Mac. `./run.sh` builds it,
// wraps it as ScrawlDemo.app (so macOS asks for its own permissions) and opens it; see README.md.
// It uses the ScrawlKit in this repository (`../..`), with ScrawlCore once
// `scripts/build-xcframework.sh` has built it.
import PackageDescription

let package = Package(
  name: "ScrawlDemoMac",
  platforms: [.macOS(.v15)],
  // Named, so the demo resolves whatever the checkout's folder is called.
  dependencies: [.package(name: "scrawl", path: "../..")],
  targets: [
    .executableTarget(
      name: "ScrawlDemoMac",
      dependencies: [.product(name: "ScrawlKit", package: "scrawl")],
      path: "App",
      swiftSettings: [.swiftLanguageMode(.v5)]
    ),
  ]
)
