import ScrawlKit
import SwiftUI

@main
struct ScrawlDemoApp: App {
  init() {
    // Build each recording into a bundle here on the phone; no Mac needed. Without `onBundle`,
    // ScrawlKit offers each new bundle in the share sheet.
    Scrawl.shared.compiles = .onDevice
  }

  var body: some Scene {
    WindowGroup {
      ContentView().scrawl()
    }
  }
}
