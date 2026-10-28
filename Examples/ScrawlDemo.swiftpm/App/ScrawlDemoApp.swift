import ScrawlKit
import SwiftUI

@main
struct ScrawlDemoApp: App {
  init() {
    // Build each recording into a bundle here on the phone; no Mac needed. Without `onBundle`,
    // ScrawlKit offers each new bundle in the share sheet.
    Scrawl.shared.compiles = .onDevice
    // Try the experiment that keeps swipes which moved nothing from cutting keyframes.
    Scrawl.shared.experiments = ["skipStillSwipes"]
  }

  var body: some Scene {
    WindowGroup {
      ContentView().scrawl()
    }
  }
}
