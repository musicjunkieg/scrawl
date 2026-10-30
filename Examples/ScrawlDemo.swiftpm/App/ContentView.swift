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
        Section {
          if let server = Scrawl.shared.serverURLs.first(where: { $0.scheme == "https" }) {
            LabeledContent("Sending to", value: server.host() ?? server.absoluteString)
            Button("Stop sending", role: .destructive) {
              Scrawl.handleSetupURL(URL(string: "scrawldemo://scrawl?reset=1")!)
            }
          } else {
            // A setup link copied on the Mac reaches here through Universal Clipboard.
            PasteButton(payloadType: String.self) { links in
              guard let link = links.first.flatMap({ URL(string: $0.trimmingCharacters(in: .whitespacesAndNewlines)) })
              else { return }
              Task { @MainActor in Scrawl.handleSetupURL(link) }
            }
          }
        } header: {
          Text("Scrawl server")
        } footer: {
          Text("Paste a setup link (scrawldemo://scrawl?url=…&token=…) to send each bundle to your scrawl-server as well.")
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
