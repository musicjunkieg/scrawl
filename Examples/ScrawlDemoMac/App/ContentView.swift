import AppKit
import ScrawlKit
import SwiftUI

/// Something to record: a counter, a text field, a list, and the bundles built so far.
struct ContentView: View {
  @State private var count = 0
  @State private var note = ""
  @State private var selection: String?
  private let fruit = ["Apples", "Pears", "Plums", "Cherries"]

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text("Press ⌃⌥⇧S (or Help › Record with Scrawl) to start. Talk, click around, hold ⇧ and drag to circle things, then press ⌃⌥⇧S again to stop.")
        .fixedSize(horizontal: false, vertical: true)
      HStack {
        Button("Clicked \(count) times") {
          count += 1
          Scrawl.event("Counter clicked (\(count))")
        }
        TextField("Type something", text: $note)
          .frame(maxWidth: 240)
      }
      List(fruit, id: \.self, selection: $selection) { Text($0) }
        .frame(height: 140)
      Divider()
      Text("Bundles built on this Mac").font(.headline)
      if Bundles.shared.all.isEmpty {
        Text("None yet").foregroundStyle(.secondary)
      }
      ForEach(Bundles.shared.all, id: \.self) { bundle in
        HStack {
          Text(bundle.lastPathComponent)
          Spacer()
          Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([bundle]) }
          Button("Open summary") { NSWorkspace.shared.open(bundle.appending(path: "summary.md")) }
        }
      }
    }
    .padding(20)
    .frame(minWidth: 560, minHeight: 420, alignment: .topLeading)
  }
}
