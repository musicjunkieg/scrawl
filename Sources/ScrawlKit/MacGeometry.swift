import CoreGraphics

/// Where a recorded Mac window sits, for ScreenCaptureKit and the timeline. AppKit's global
/// coordinates start at the main display's bottom-left corner with y going up. ScreenCaptureKit's
/// `sourceRect` starts at its display's top-left corner, and the timeline at the recorded area's,
/// both with y going down.
enum MacGeometry {
  /// The part of `window` on `display` (both in global AppKit points): what gets recorded.
  static func area(of window: CGRect, on display: CGRect) -> CGRect {
    let visible = window.intersection(display)
    return visible.isNull ? .zero : visible.integral
  }

  /// `area` (global AppKit points) as a `sourceRect` on the display whose frame is `display`.
  static func sourceRect(of area: CGRect, on display: CGRect) -> CGRect {
    CGRect(x: area.minX - display.minX, y: display.maxY - area.maxY, width: area.width, height: area.height)
  }

  /// A global AppKit point in the recorded area's points, from its top-left corner.
  static func point(_ point: CGPoint, in area: CGRect) -> CGPoint {
    CGPoint(x: point.x - area.minX, y: area.maxY - point.y)
  }

  /// The movie's size in pixels for `size` points at `scale`, both sides even (as H.264 wants).
  static func pixels(_ size: CGSize, scale: CGFloat) -> (width: Int, height: Int) {
    func even(_ points: CGFloat) -> Int { max(2, Int((points * scale).rounded()) & ~1) }
    return (even(size.width), even(size.height))
  }

  /// Where the recorded area goes when its window moves to `window`: the same size, at the
  /// window's top-left corner.
  static func moved(_ area: CGRect, to window: CGRect) -> CGRect {
    CGRect(x: window.minX, y: window.maxY - area.height, width: area.width, height: area.height)
  }
}
