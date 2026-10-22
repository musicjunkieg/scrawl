import CoreGraphics
import Foundation
@testable import ScrawlKit
import Testing

struct InkTests {
  /// Deterministic white noise (SplitMix64) of ±0.8 pt standing in for finger jitter.
  func jitter(_ i: Int) -> Double {
    var z = UInt64(i + 1) &* 0x9E37_79B9_7F4A_7C15
    z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
    z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
    z ^= z >> 31
    return (Double(z >> 11) / Double(1 << 53) * 2 - 1) * 0.8
  }

  // MARK: Filter

  @Test func oneEuroRemovesJitterFromASlowLine() {
    // A slow horizontal line with jitter across it: lag along the line doesn't change its shape,
    // wobble across it does.
    var filter = OneEuroFilter(minCutoff: 3, beta: 0.03)
    var rawError = 0.0
    var filteredError = 0.0
    for i in 0 ..< 240 {
      let raw = CGPoint(x: Double(i) / 3, y: jitter(i))
      let filtered = filter.filter(raw, at: Double(i) / 120)
      if i > 30 {
        rawError += abs(raw.y)
        filteredError += abs(filtered.y)
      }
    }
    #expect(filteredError < rawError * 0.5)
  }

  @Test func oneEuroBarelyLagsAFastLine() {
    var filter = OneEuroFilter(minCutoff: 3, beta: 0.03)
    var value = CGPoint.zero
    for i in 0 ... 60 {
      value = filter.filter(CGPoint(x: Double(i) * 10, y: 0), at: Double(i) / 120) // 1200 points a second
    }
    #expect(abs(value.x - 600) < 12)
  }

  @Test func aDiagonalStaysOnItsLine() {
    // Separate x and y filters lag by different amounts when the axes move at different speeds,
    // which pulled the end of an arrow into a hook.
    var filter = OneEuroFilter(minCutoff: 3, beta: 0.03)
    for i in 0 ... 60 {
      let p = filter.filter(CGPoint(x: Double(i) * 4, y: Double(i) * 1.2), at: Double(i) / 120)
      #expect(abs(p.y - p.x * 0.3) < 1e-6)
    }
  }

  @Test func oneEuroHoldsStillAndSurvivesRepeatedTimestamps() {
    var filter = OneEuroFilter(minCutoff: 3, beta: 0.03)
    for _ in 0 ..< 10 {
      #expect(filter.filter(CGPoint(x: 5, y: 7), at: 1) == CGPoint(x: 5, y: 7))
    }
  }

  @Test func theLineBehindTheTipIsSmoothedButItsEndsStayPut() {
    let wobbly = (0 ..< 80).map { CGPoint(x: Double($0) * 2, y: jitter($0)) }
    var trace = InkTrace()
    trace.append(samples(wobbly))
    let smoothed = trace.smoothedSamples
    #expect(smoothed.first?.point == wobbly.first)
    #expect(smoothed.last?.point == wobbly.last)
    let rawWobble = wobbly.dropFirst().dropLast().map { abs($0.y) }.reduce(0, +)
    let smoothWobble = smoothed.dropFirst().dropLast().map { abs($0.point.y) }.reduce(0, +)
    #expect(smoothWobble < rawWobble * 0.6)
    // A straight line stays exactly straight.
    var straight = InkTrace()
    straight.append(samples((0 ..< 20).map { CGPoint(x: Double($0) * 3, y: Double($0)) }))
    #expect(straight.smoothedSamples.allSatisfy { abs($0.point.y - $0.point.x / 3) < 1e-9 })
  }

  // MARK: Builder

  @Test func samplesAreSpacedAndEndWhereTheFingerLifted() {
    var builder = InkStrokeBuilder()
    for i in 0 ..< 200 {
      builder.add(CGPoint(x: Double(i) * 0.1, y: 0), at: Double(i) / 240)
    }
    builder.finish(at: CGPoint(x: 20, y: 0), time: 1)
    let samples = builder.samples
    for (a, b) in zip(samples, samples.dropFirst()) {
      #expect(hypot(b.point.x - a.point.x, b.point.y - a.point.y) >= InkStrokeBuilder.minSpacing - 1e-9)
      #expect(b.time >= a.time)
    }
    #expect(samples.last?.point == CGPoint(x: 20, y: 0))
  }

  @Test func fastLinesAreThinnerButNeverPressureBased() {
    #expect(InkStrokeBuilder.width(forSpeed: 0) == InkStrokeBuilder.maxWidth)
    #expect(InkStrokeBuilder.width(forSpeed: 400) > InkStrokeBuilder.width(forSpeed: 1200))
    let thinnest = InkStrokeBuilder.width(forSpeed: 10000)
    #expect(abs(thinnest - InkStrokeBuilder.maxWidth * (1 - InkStrokeBuilder.thinning)) < 1e-9)
  }

  @Test func predictionsContinueTheSmoothedLineWithoutChangingIt() {
    var builder = InkStrokeBuilder()
    for i in 0 ..< 60 {
      builder.add(CGPoint(x: Double(i) * 2, y: 0), at: Double(i) / 120)
    }
    let before = builder.samples
    let ahead = builder.predicting([(CGPoint(x: 122, y: 0), 0.5), (CGPoint(x: 124, y: 0), 0.51)])
    #expect(builder.samples == before)
    #expect(!ahead.isEmpty)
    #expect(ahead.allSatisfy { $0.point.x > before.last!.point.x })
  }

  // MARK: Geometry

  func samples(_ points: [CGPoint], width: CGFloat = 4) -> [InkSample] {
    points.enumerated().map { InkSample(point: $1, time: Double($0) / 120, width: width) }
  }

  func circle(_ count: Int, radius: CGFloat = 60) -> [CGPoint] {
    (0 ..< count).map { i in
      let a = Double(i) / Double(count) * 2 * .pi * 1.1
      return CGPoint(x: 100 + radius * cos(a), y: 100 + radius * sin(a))
    }
  }

  @Test func theCurvePassesThroughEverySample() {
    let p = [CGPoint(x: 0, y: 0), CGPoint(x: 10, y: 3), CGPoint(x: 18, y: 12), CGPoint(x: 21, y: 30)]
    #expect(InkTrace.catmullRom(p[0], p[1], p[2], p[3], 0) == p[1])
    let end = InkTrace.catmullRom(p[0], p[1], p[2], p[3], 1)
    #expect(abs(end.x - p[2].x) < 1e-9 && abs(end.y - p[2].y) < 1e-9)
  }

  @Test func drawingItBitByBitGivesTheSameOutlineAsAllAtOnce() {
    let all = samples(circle(90))
    var trace = InkTrace()
    var drawn: [InkTrace.Point] = []
    for sample in all {
      trace.append([sample])
      drawn += trace.freeze()
      // The live tip never reaches back into what's frozen.
      if let tip = trace.tail().first, let last = drawn.last { #expect(tip == last) }
    }
    #expect(trace.frozen > 60)
    let tail = trace.tail(finished: true)
    #expect(tail.first == drawn.last)
    // Each batch of frozen segments starts at the previous batch's last point (a shared disc).
    let joined = (drawn + tail.dropFirst()).reduce(into: [InkTrace.Point]()) { points, point in
      if points.last != point { points.append(point) }
    }
    #expect(joined == trace.outline)
  }

  @Test func predictedPointsOnlyExtendTheTip() {
    var trace = InkTrace()
    trace.append(samples(circle(40)))
    _ = trace.freeze()
    let predicted = samples([CGPoint(x: 180, y: 140), CGPoint(x: 182, y: 150)])
    let live = trace.tail(predicted: predicted)
    #expect(live.count > trace.tail().count)
    #expect(trace.samples.count == 40)
  }

  @Test func theOutlineCoversTheLineAndNothingFarFromIt() {
    var trace = InkTrace()
    trace.append(samples(circle(120)))
    let points = trace.outline
    let path = InkTrace.path(points)
    for point in points {
      #expect(path.contains(point.point, using: .winding))
    }
    #expect(!path.contains(CGPoint(x: 100, y: 100), using: .winding))
    #expect(!path.contains(CGPoint(x: 100, y: 100 + 60 + 6), using: .winding))
  }

  @Test func hairpinsAndCrossingsLeaveNoHoles() {
    // Down, then straight back up over itself, then across: every overlap must stay filled.
    // Typed in parts: as one expression it times out Swift's type checker.
    let down: [CGPoint] = (0 ..< 40).map { CGPoint(x: 50, y: Double($0) * 2) }
    let up: [CGPoint] = (0 ..< 40).map { CGPoint(x: 50.5, y: 78 - Double($0) * 2) }
    let across: [CGPoint] = (0 ..< 40).map { CGPoint(x: 10 + Double($0) * 2, y: 40) }
    let line = down + up + across
    var trace = InkTrace()
    trace.append(samples(line, width: 4))
    let path = InkTrace.path(trace.outline)
    for y in stride(from: 4.0, through: 74, by: 2) {
      #expect(path.contains(CGPoint(x: 50.25, y: y), using: .winding))
    }
    #expect(path.contains(CGPoint(x: 50.25, y: 40), using: .winding))
    #expect(!path.contains(CGPoint(x: 60, y: 60), using: .winding))
  }

  @Test func theEndsTaper() {
    var trace = InkTrace()
    trace.append(samples((0 ..< 60).map { CGPoint(x: Double($0), y: 0) }, width: 4))
    let outline = trace.outline
    #expect(abs(outline.first!.radius - 2 * InkTrace.taperTip) < 1e-6)
    #expect(abs(outline.last!.radius - 2 * InkTrace.taperTip) < 1e-6)
    #expect(outline.map(\.radius).max()! == 2)
    // While drawing, the tip isn't tapered (it would look like the pen shrinking).
    #expect(trace.tail().last!.radius == 2)
  }

  @Test func aTapIsADot() {
    var trace = InkTrace()
    trace.append(samples([CGPoint(x: 5, y: 5)]))
    let path = InkTrace.path(trace.outline)
    #expect(path.contains(CGPoint(x: 5, y: 5), using: .winding))
    #expect(!path.contains(CGPoint(x: 8, y: 5), using: .winding))
  }

  // MARK: Fading

  @Test func aStrokeFadesAfterTheHold() {
    var fade = InkFade()
    #expect(fade.penDown(1, at: 0) == nil)
    #expect(fade.opacity(of: 1, at: 5) == 1)
    fade.penUp(at: 5)
    #expect(fade.fadeAt == 5 + InkFade.hold)
    #expect(fade.advance(to: 5 + InkFade.hold - 0.01) == nil)
    #expect(fade.opacity(of: 1, at: 5 + InkFade.hold - 0.01) == 1)
    let group = fade.advance(to: 5 + InkFade.hold)
    #expect(group == .init(strokes: [1], start: 5 + InkFade.hold))
    let start = 5 + InkFade.hold
    #expect(fade.opacity(of: 1, at: start) == 1)
    let a = fade.opacity(of: 1, at: start + 0.1)
    let b = fade.opacity(of: 1, at: start + 0.2)
    #expect(a < 1 && b < a && b > 0)
    // Ease-out: most of it goes early.
    #expect(a < 0.5)
    #expect(fade.opacity(of: 1, at: start + InkFade.duration) < 1e-9)
    #expect(fade.removeFaded(at: start + InkFade.duration - 0.01).isEmpty)
    #expect(fade.removeFaded(at: start + InkFade.duration) == [1])
    #expect(fade.fading.isEmpty)
  }

  @Test func strokesInQuickSuccessionFadeTogether() {
    var fade = InkFade()
    _ = fade.penDown(1, at: 0) // a circle
    fade.penUp(at: 0.8)
    // The arrow starts before the circle's hold runs out: both are held while it's drawn.
    #expect(fade.penDown(2, at: 1.5) == nil)
    #expect(fade.advance(to: 4) == nil)
    #expect(fade.opacity(of: 1, at: 4) == 1)
    fade.penUp(at: 4.2)
    #expect(fade.advance(to: 4.2 + InkFade.hold) == .init(strokes: [1, 2], start: 4.2 + InkFade.hold))
  }

  @Test func aStrokeAfterTheGroupBeganFadingStartsANewGroup() {
    var fade = InkFade()
    _ = fade.penDown(1, at: 0)
    fade.penUp(at: 1)
    // The timer hadn't fired yet when the pen came down again: the old group goes anyway.
    let late = fade.penDown(2, at: 1 + InkFade.hold + 0.05)
    #expect(late == .init(strokes: [1], start: 1 + InkFade.hold))
    #expect(fade.held == [2])
    fade.penUp(at: 4)
    #expect(fade.advance(to: 4 + InkFade.hold)?.strokes == [2])
  }
}
