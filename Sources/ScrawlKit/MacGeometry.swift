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

  /// Where the recorded area goes when its window moves from `old` to `new` (global AppKit
  /// points): along with the window, the same size, kept on the recorded `display`.
  static func moved(_ area: CGRect, from old: CGRect, to new: CGRect, on display: CGRect) -> CGRect {
    let moved = area.offsetBy(dx: new.minX - old.minX, dy: new.maxY - old.maxY)
    let x = min(max(moved.minX, display.minX), display.maxX - moved.width)
    let y = min(max(moved.minY, display.minY), display.maxY - moved.height)
    return CGRect(x: x, y: y, width: moved.width, height: moved.height)
  }
}
