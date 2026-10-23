#if os(iOS)
  import SwiftUI
  import UIKit

  /// A window above the app for freehand red strokes and the recording strip. It only exists from
  /// the start of a recording until the strip has animated away after it.
  /// ReplayKit recordings include it; app-frame recordings leave it out. Every touch except one on
  /// the strip or its bubbles (or single-finger drawing when `isDrawing` is active) passes through
  /// to the app below.
  final class OverlayWindow: UIWindow {
    private var regions: [OverlayRegion: CGRect] = [:]
    private unowned let scrawl: Scrawl
    private var drawingTouch: UITouch?
    var isActivelyDrawing: Bool { drawingTouch != nil }
    /// The red ink, under the strip. UIKit layers rather than SwiftUI, so a touch redraws only the
    /// stroke's live tip.
    let ink = InkView()

    init(windowScene: UIWindowScene, scrawl: Scrawl) {
      self.scrawl = scrawl
      super.init(windowScene: windowScene)
      windowLevel = .alert + 10
      backgroundColor = .clear
      let host = UIHostingController(rootView: OverlayView(scrawl: scrawl) { [weak self] region, frame in
        self?.regions[region] = frame.isEmpty ? nil : frame
      })
      host.view.backgroundColor = .clear
      rootViewController = host
      ink.frame = bounds
      // Index 0: under the root view whenever UIKit adds it.
      insertSubview(ink, at: 0)
      ink.onStroke = { [unowned scrawl] samples in scrawl.record(stroke: samples) }
      ink.onFade = { [unowned scrawl] strokes, uptime in scrawl.recordFade(of: strokes, at: uptime) }
      isHidden = false
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) { fatalError() }

    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
      if regions.values.contains(where: { $0.insetBy(dx: -8, dy: -8).contains(point) }) {
        return super.hitTest(point, with: event)
      }
      if scrawl.isRecording, scrawl.isDrawing {
        return self
      }
      return nil
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
      // One finger draws at a time; others are ignored until it lifts.
      guard scrawl.isRecording, scrawl.isDrawing, drawingTouch == nil, let touch = touches.first else {
        if drawingTouch == nil { super.touchesBegan(touches, with: event) }
        return
      }
      drawingTouch = touch
      ink.began(touch, with: event)
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesMoved(touches, with: event)
        return
      }
      ink.moved(active, with: event)
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesEnded(touches, with: event)
        return
      }
      ink.ended(active, with: event)
      drawingTouch = nil
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
      guard let active = drawingTouch, touches.contains(active) else {
        super.touchesCancelled(touches, with: event)
        return
      }
      ink.ended(active, with: event)
      drawingTouch = nil
    }

    /// Removes the ink (a recording started or finished).
    func clearInk(animated: Bool) {
      drawingTouch = nil
      ink.clear(animated: animated)
    }
  }

  private struct OverlayView: View {
    let scrawl: Scrawl
    let reportFrame: @MainActor (OverlayRegion, CGRect) -> Void

    var body: some View {
      // The ink is `OverlayWindow.ink`, under this view.
      StripPlacement(mode: stripMode, actions: actions, notice: scrawl.isRecording ? scrawl.notice : nil)
    }

    private var stripMode: ScrawlStrip.Mode? {
      if scrawl.isRecording, let startedAt = scrawl.startedAt {
        return .recording(startedAt: startedAt, isDrawing: scrawl.isDrawing)
      }
      if scrawl.isDiscardPending {
        return .discarded
      }
      switch scrawl.uploadStatus {
      case .idle: return nil
      case .sending:
        return .sending(.init(progress: scrawl.sendProgress, stalled: scrawl.isSendStalled, building: scrawl.isBuildingOnPhone))
      case let .sent(target): return .sent(target)
      case .queued: return .queued
      case .failed: return .failed(scrawl.deliveryFailure)
      }
    }

    private var actions: ScrawlStrip.Actions {
      ScrawlStrip.Actions(
        toggleDrawing: { scrawl.toggleDrawing() },
        discard: { scrawl.discard() },
        send: {
          UIImpactFeedbackGenerator(style: .light).impactOccurred()
          scrawl.stop()
        },
        undoDiscard: { scrawl.undoDiscard() },
        share: { scrawl.shareFailedRecording() },
        reportFrame: reportFrame
      )
    }
  }
#endif
