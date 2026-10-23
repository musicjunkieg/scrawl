// swift-tools-version: 5.9
// A small app to try ScrawlKit on an iPhone, building bundles on the phone (`compiles =
// .onDevice`). Open this folder in Xcode, pick your iPhone and Run; see README.md. It uses the
// ScrawlKit in this repository (`../..`), with ScrawlCore once `scripts/build-xcframework.sh` has
// built it.
import AppleProductTypes
import PackageDescription

let package = Package(
  name: "ScrawlDemo",
  platforms: [.iOS("18.0")],
  products: [
    .iOSApplication(
      name: "ScrawlDemo",
      targets: ["ScrawlDemo"],
      bundleIdentifier: "dev.scrawl.demo",
      displayVersion: "1.0",
      bundleVersion: "1",
      appIcon: .placeholder(icon: .pencil),
      accentColor: .presetColor(.red),
      supportedDeviceFamilies: [.phone],
      supportedInterfaceOrientations: [.portrait],
      capabilities: [
        .microphone(purposeString: "Scrawl records what you say while you show the problem."),
        .speechRecognition(purposeString: "Scrawl turns what you said into text, on this iPhone."),
      ]
    ),
  ],
  dependencies: [.package(path: "../..")],
  targets: [
    .executableTarget(
      name: "ScrawlDemo",
      dependencies: [.product(name: "ScrawlKit", package: "scrawl")],
      path: "App"
    ),
  ]
)
