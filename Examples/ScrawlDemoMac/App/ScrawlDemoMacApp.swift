import ScrawlKit
import SwiftUI

@main
struct ScrawlDemoMacApp: App {
  init() {
    // Show each bundle in a window as it's built (instead of in the Finder).
    Scrawl.shared.onBundle = { bundle in Bundles.shared.add(bundle) }
  }

  var body: some Scene {
    WindowGroup("Scrawl demo") {
      ContentView()
        .scrawl()
        .scrawlScreen("Demo")
    }
  }
}

/// The bundles built since launch, newest first.
@MainActor
@Observable
final class Bundles {
  static let shared = Bundles()
  private(set) var all: [URL] = []

  func add(_ bundle: URL) {
    all.insert(bundle, at: 0)
  }
}
