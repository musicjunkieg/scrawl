import CoreGraphics
@testable import ScrawlKit
import Testing

struct MacGeometryTests {
  /// A 1512×982 main display, and a second one above it.
  let main = CGRect(x: 0, y: 0, width: 1512, height: 982)
  let above = CGRect(x: 0, y: 982, width: 1920, height: 1080)

  @Test func aWindowIsRecordedFromItsDisplaysTopLeft() {
    let window = CGRect(x: 100, y: 200, width: 800, height: 600)
    let area = MacGeometry.area(of: window, on: main)
    #expect(area == window)
    // Its top edge is at y 800, 182 points below the display's top.
    #expect(MacGeometry.sourceRect(of: area, on: main) == CGRect(x: 100, y: 182, width: 800, height: 600))
    #expect(MacGeometry.sourceRect(of: CGRect(x: 10, y: 1000, width: 100, height: 100), on: above)
      == CGRect(x: 10, y: 962, width: 100, height: 100))
  }

  @Test func onlyThePartOnTheDisplayIsRecorded() {
    let hanging = CGRect(x: -50, y: 700, width: 400, height: 400)
    #expect(MacGeometry.area(of: hanging, on: main) == CGRect(x: 0, y: 700, width: 350, height: 282))
    #expect(MacGeometry.area(of: CGRect(x: 3000, y: 0, width: 10, height: 10), on: main) == .zero)
  }

  @Test func clicksAreMeasuredFromTheAreasTopLeft() {
    let area = CGRect(x: 100, y: 200, width: 800, height: 600)
    #expect(MacGeometry.point(CGPoint(x: 100, y: 800), in: area) == CGPoint(x: 0, y: 0))
    #expect(MacGeometry.point(CGPoint(x: 900, y: 200), in: area) == CGPoint(x: 800, y: 600))
    #expect(MacGeometry.point(CGPoint(x: 150, y: 750), in: area) == CGPoint(x: 50, y: 50))
  }

  @Test func moviesHaveEvenSides() {
    let retina = MacGeometry.pixels(CGSize(width: 800, height: 600), scale: 2)
    #expect(retina.width == 1600 && retina.height == 1200)
    let odd = MacGeometry.pixels(CGSize(width: 801, height: 601), scale: 1)
    #expect(odd.width == 800 && odd.height == 600)
    let tiny = MacGeometry.pixels(CGSize(width: 0.4, height: 0), scale: 1)
    #expect(tiny.width == 2 && tiny.height == 2)
  }

  @Test func aMovedWindowTakesItsAreaAlong() {
    let window = CGRect(x: 100, y: 200, width: 800, height: 600)
    let area = MacGeometry.area(of: window, on: main)
    // Moved and made taller: the area stays 800×600, hung from the new top edge.
    let moved = MacGeometry.moved(area, from: window, to: CGRect(x: 300, y: 100, width: 900, height: 800), on: main)
    #expect(moved == CGRect(x: 300, y: 300, width: 800, height: 600))
  }

  @Test func aWindowHangingOffTheDisplayKeepsItsOffset() {
    // Hanging 50 points off the left edge: the area starts 50 points into the window.
    let window = CGRect(x: -50, y: 100, width: 400, height: 300)
    let area = MacGeometry.area(of: window, on: main)
    #expect(area == CGRect(x: 0, y: 100, width: 350, height: 300))
    let moved = MacGeometry.moved(area, from: window, to: window.offsetBy(dx: 60, dy: 0), on: main)
    #expect(moved == CGRect(x: 60, y: 100, width: 350, height: 300))
  }

  @Test func anAreaMovedOffTheDisplayStaysOnIt() {
    let window = CGRect(x: 100, y: 200, width: 800, height: 600)
    let area = MacGeometry.area(of: window, on: main)
    // Dragged toward the display above: the area stops at the recorded display's top edge.
    let moved = MacGeometry.moved(area, from: window, to: window.offsetBy(dx: 0, dy: 500), on: main)
    #expect(moved == CGRect(x: 100, y: 382, width: 800, height: 600))
  }

  @Test func anAreaHeldAtTheEdgeComesBackWithItsWindow() {
    let window = CGRect(x: 100, y: 200, width: 800, height: 600)
    let area = MacGeometry.area(of: window, on: main)
    // Off the left edge and back: measured from the start each time, the area returns.
    let away = MacGeometry.moved(area, from: window, to: window.offsetBy(dx: -200, dy: 0), on: main)
    #expect(away.minX == 0)
    let back = MacGeometry.moved(area, from: window, to: window, on: main)
    #expect(back == area)
  }
}
