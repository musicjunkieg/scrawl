#if os(macOS)
  import AVFoundation
  import CoreMedia
  import ScreenCaptureKit

  /// Records part of one display into an MP4 with ScreenCaptureKit: only this app's windows (less
  /// the ones left out, Scrawl's own overlay), the pointer, and the microphone when narrating.
  /// Frames arrive only when something on screen changes; the movie holds the last one until the
  /// recording stops.
  ///
  /// Times are host time in seconds (`CACurrentMediaTime`, `NSEvent.timestamp`): the movie starts
  /// at its first frame, which `firstFrame` reports.
  final class MacScreenRecorder: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    struct Failure: LocalizedError {
      let description: String
      var errorDescription: String? { description }
    }

    /// What to record: a display, the rectangle on it (its own points, from its top-left
    /// corner), and its pixels per point.
    struct Area {
      let displayID: CGDirectDisplayID
      let sourceRect: CGRect
      let scale: CGFloat
    }

    let movie: URL
    /// The capture stopped on its own (the display went away, permission was taken back).
    var onStop: (@Sendable (Error) -> Void)?

    // Everything below is touched on `queue` once capture starts.
    private let queue = DispatchQueue(label: "dev.kitlangton.scrawl.mac-recorder")
    private var stream: SCStream?
    private var configuration: SCStreamConfiguration?
    private var writer: AVAssetWriter?
    private var video: AVAssetWriterInput?
    private var audio: AVAssetWriterInput?
    private var sessionStart: CMTime?
    private var firstFrame: (@Sendable (Double) -> Void)?
    private var audioWritten = false

    init(movie: URL) {
      self.movie = movie
    }

    /// Starts recording `area`, leaving out the windows `leavingOut` names. `firstFrame` gets the
    /// host time the movie starts at, once its first frame is written.
    func start(
      area: Area, leavingOut windowIDs: Set<CGWindowID>, narration: Bool,
      firstFrame: @escaping @Sendable (Double) -> Void
    ) async throws {
      let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
      guard let display = content.displays.first(where: { $0.displayID == area.displayID }) else {
        throw Failure(description: "The window's display can't be recorded")
      }
      guard let app = content.applications.first(where: { $0.processID == getpid() }) else {
        throw Failure(description: "This app's windows can't be recorded")
      }
      let leftOut = content.windows.filter { windowIDs.contains($0.windowID) }
      let filter = SCContentFilter(display: display, including: [app], exceptingWindows: leftOut)

      let pixels = MacGeometry.pixels(area.sourceRect.size, scale: area.scale)
      let configuration = SCStreamConfiguration()
      configuration.sourceRect = area.sourceRect
      configuration.width = pixels.width
      configuration.height = pixels.height
      configuration.minimumFrameInterval = CMTime(value: 1, timescale: 30)
      configuration.pixelFormat = kCVPixelFormatType_32BGRA
      configuration.showsCursor = true
      configuration.queueDepth = 6
      configuration.capturesAudio = false
      configuration.captureMicrophone = narration

      try? FileManager.default.removeItem(at: movie)
      let writer = try AVAssetWriter(outputURL: movie, fileType: .mp4)
      // About 2 bits a pixel each second: sharp text, and a few MB a minute for a laptop window.
      let bitRate = min(max(pixels.width * pixels.height * 2, 2_000_000), 12_000_000)
      let video = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: pixels.width,
        AVVideoHeightKey: pixels.height,
        AVVideoCompressionPropertiesKey: [
          AVVideoAverageBitRateKey: bitRate,
          AVVideoExpectedSourceFrameRateKey: 30,
          AVVideoMaxKeyFrameIntervalKey: 60,
        ],
      ])
      video.expectsMediaDataInRealTime = true
      writer.add(video)
      var audio: AVAssetWriterInput?
      if narration {
        let input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
          AVFormatIDKey: kAudioFormatMPEG4AAC,
          AVNumberOfChannelsKey: 1,
          AVSampleRateKey: 48_000,
          AVEncoderBitRateKey: 96_000,
        ])
        input.expectsMediaDataInRealTime = true
        if writer.canAdd(input) {
          writer.add(input)
          audio = input
        }
      }
      guard writer.startWriting() else {
        throw writer.error ?? Failure(description: "The movie couldn't be started")
      }

      let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
      try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
      if audio != nil {
        try stream.addStreamOutput(self, type: .microphone, sampleHandlerQueue: queue)
      }
      queue.sync {
        self.stream = stream
        self.configuration = configuration
        self.writer = writer
        self.video = video
        self.audio = audio
        self.firstFrame = firstFrame
      }
      do {
        try await stream.startCapture()
      } catch {
        writer.cancelWriting()
        throw error
      }
    }

    /// Moves the recorded rectangle (same size) when its window moves.
    func move(to sourceRect: CGRect) {
      queue.async {
        guard let stream = self.stream, let configuration = self.configuration else { return }
        configuration.sourceRect = sourceRect
        stream.updateConfiguration(configuration) { error in
          if let error { print("Scrawl: couldn't follow the window: \(error.localizedDescription)") }
        }
      }
    }

    /// Whether the movie has sound.
    var recordsAudio: Bool { queue.sync { audioWritten } }

    /// Stops capturing and finishes the movie, which runs until now.
    func stop() async throws {
      let stream = queue.sync { self.stream }
      try? await stream?.stopCapture()
      let end = CMClockGetTime(CMClockGetHostTimeClock())
      let writer: AVAssetWriter? = await withCheckedContinuation { continuation in
        queue.async {
          defer { self.stream = nil }
          guard let writer = self.writer, writer.status == .writing else {
            continuation.resume(returning: nil)
            return
          }
          guard let start = self.sessionStart else {
            writer.cancelWriting()
            continuation.resume(returning: nil)
            return
          }
          self.video?.markAsFinished()
          self.audio?.markAsFinished()
          writer.endSession(atSourceTime: max(end, start))
          continuation.resume(returning: writer)
        }
      }
      guard let writer else { throw Failure(description: "Nothing was recorded") }
      await writer.finishWriting()
      if writer.status != .completed {
        throw writer.error ?? Failure(description: "The movie couldn't be finished")
      }
    }

    // MARK: SCStreamOutput, on `queue`

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
      guard buffer.isValid, let writer, writer.status == .writing else { return }
      switch type {
      case .screen:
        guard Self.isComplete(buffer), let video else { return }
        let time = buffer.presentationTimeStamp
        if sessionStart == nil {
          writer.startSession(atSourceTime: time)
          sessionStart = time
          firstFrame?(time.seconds)
          firstFrame = nil
        }
        if video.isReadyForMoreMediaData {
          video.append(buffer)
        }
      case .microphone:
        guard let sessionStart, let audio, audio.isReadyForMoreMediaData,
              buffer.presentationTimeStamp >= sessionStart
        else { return }
        if audio.append(buffer) { audioWritten = true }
      default:
        break
      }
    }

    /// A frame with new pixels (ScreenCaptureKit also sends idle and blank ones).
    private static func isComplete(_ buffer: CMSampleBuffer) -> Bool {
      guard let attachments = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: false)
        as? [[SCStreamFrameInfo: Any]],
        let raw = attachments.first?[.status] as? Int,
        let status = SCFrameStatus(rawValue: raw)
      else { return false }
      return status == .complete
    }

    // MARK: SCStreamDelegate

    func stream(_ stream: SCStream, didStopWithError error: any Error) {
      onStop?(error)
    }
  }
#endif
