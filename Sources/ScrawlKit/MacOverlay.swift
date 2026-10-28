#if os(macOS)
  import AppKit

  /// A see-through panel over the recorded window: red ink while ⇧ is held, and a line saying
  /// what Scrawl is doing. It never takes the mouse (Scrawl reads the app's own mouse events)
  /// and is left out of the recording: the compiler draws the ink from the timeline.
  final class MacOverlay: NSPanel {
    private let canvas = Canvas()
    private let status = NSTextField(labelWithString: "")
    private let statusBackground = NSVisualEffectView()

    convenience init(over area: CGRect) {
      self.init(contentRect: area, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      isOpaque = false
      backgroundColor = .clear
      hasShadow = false
      ignoresMouseEvents = true
      isReleasedWhenClosed = false
      level = .floating
      collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle, .stationary]

      let root = NSView(frame: CGRect(origin: .zero, size: area.size))
      root.autoresizingMask = [.width, .height]
      canvas.frame = root.bounds
      canvas.autoresizingMask = [.width, .height]
      root.addSubview(canvas)

      statusBackground.material = .hudWindow
      statusBackground.blendingMode = .behindWindow
      statusBackground.state = .active
      statusBackground.wantsLayer = true
      statusBackground.layer?.cornerRadius = 9
      statusBackground.translatesAutoresizingMaskIntoConstraints = false
      status.font = .systemFont(ofSize: 12, weight: .medium)
      status.textColor = .labelColor
      status.translatesAutoresizingMaskIntoConstraints = false
      statusBackground.addSubview(status)
      root.addSubview(statusBackground)
      NSLayoutConstraint.activate([
        statusBackground.centerXAnchor.constraint(equalTo: root.centerXAnchor),
        statusBackground.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
        status.leadingAnchor.constraint(equalTo: statusBackground.leadingAnchor, constant: 12),
        status.trailingAnchor.constraint(equalTo: statusBackground.trailingAnchor, constant: -12),
        status.topAnchor.constraint(equalTo: statusBackground.topAnchor, constant: 6),
        status.bottomAnchor.constraint(equalTo: statusBackground.bottomAnchor, constant: -6),
      ])
      contentView = root
    }

    /// The window number ScreenCaptureKit knows this panel by.
    var windowID: CGWindowID { CGWindowID(windowNumber) }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    func show(_ text: String?) {
      status.stringValue = text ?? ""
      statusBackground.isHidden = text == nil
      orderFrontRegardless()
    }

    /// Follows the recorded area (global AppKit points).
    func move(to area: CGRect) {
      setFrame(area, display: true)
    }

    // MARK: Ink, in the area's points from its top-left corner

    func beginStroke(at point: CGPoint) {
      canvas.strokes.append(Canvas.Stroke(points: [point]))
      canvas.needsDisplay = true
    }

    func extendStroke(to point: CGPoint) {
      guard !canvas.strokes.isEmpty else { return }
      canvas.strokes[canvas.strokes.count - 1].points.append(point)
      canvas.needsDisplay = true
    }

    /// Ends the stroke being drawn; it fades away after a few seconds, as on the Mac.
    func endStroke() {
      guard let index = canvas.strokes.indices.last else { return }
      let id = canvas.strokes[index].id
      DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in
        guard let canvas = self?.canvas else { return }
        canvas.strokes.removeAll { $0.id == id }
        canvas.needsDisplay = true
      }
    }

    func clearInk() {
      canvas.strokes = []
      canvas.needsDisplay = true
    }

    private final class Canvas: NSView {
      struct Stroke {
        let id = UUID()
        var points: [CGPoint]
      }

      var strokes: [Stroke] = []

      override var isFlipped: Bool { true }

      override func draw(_ dirtyRect: NSRect) {
        NSColor.systemRed.setStroke()
        for stroke in strokes {
          guard let first = stroke.points.first else { continue }
          let path = NSBezierPath()
          path.lineWidth = 4
          path.lineCapStyle = .round
          path.lineJoinStyle = .round
          path.move(to: first)
          if stroke.points.count == 1 {
            path.line(to: first)
          }
          for point in stroke.points.dropFirst() {
            path.line(to: point)
          }
          path.stroke()
        }
      }
    }
  }
#endif
