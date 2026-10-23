import ScrawlKit
import SwiftUI

/// Something to record: a counter, a text field, and the bundles built so far.
struct ContentView: View {
  @State private var count = 0
  @State private var note = ""
  @State private var bundles: [URL] = []
  @Environment(\.scenePhase) private var phase

  var body: some View {
    NavigationStack {
      List {
        Section {
          Text("Three-finger long press to start recording. Talk, tap around, turn on Draw to circle things, then press again (or Send) to stop.")
            .font(.callout)
        }
        Section("Something to show") {
          Button("Tapped \(count) times") {
            count += 1
            Scrawl.event("Counter tapped (\(count))")
          }
          TextField("Type something", text: $note)
        }
        Section("Bundles on this iPhone") {
          if bundles.isEmpty {
            Text("None yet").foregroundStyle(.secondary)
          }
          ForEach(bundles, id: \.self) { bundle in
            ShareLink(item: bundle) {
              Label(bundle.lastPathComponent, systemImage: "doc.richtext")
            }
          }
        }
      }
      .navigationTitle("Scrawl demo")
      .refreshable { load() }
      .onAppear { load() }
      .onChange(of: phase) { load() }
      // "Saved on this iPhone": a new bundle is in the folder.
      .onChange(of: Scrawl.shared.uploadStatus) { load() }
    }
  }

  /// The `<stamp>.scrawl` folders in ScrawlKit's directory, newest first.
  private func load() {
    let files = (try? FileManager.default.contentsOfDirectory(
      at: Scrawl.directory, includingPropertiesForKeys: nil
    )) ?? []
    bundles = files.filter { $0.pathExtension == "scrawl" }
      .sorted { $0.lastPathComponent > $1.lastPathComponent }
  }
}
