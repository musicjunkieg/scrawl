#if os(macOS)
  import AppKit
  import AVFoundation
  import SwiftUI

  /// Records your Mac app's window with narration, the clicks that drove it (each with the label
  /// of what was clicked), markers, red ink and app events, all on one clock, and turns the
  /// recording into a `.scrawl` bundle for an agent.
  ///
  ///     WindowGroup { ContentView().scrawl() }   // ⌃⌥⇧S, or Help › Record with Scrawl
  ///     Scrawl.event("Saved")                    // optional: what the app did, on the timeline
  ///
  /// The shortcut (or the Help menu item) starts recording the window in front, and again stops
  /// and sends it. Hold ⇧ and drag to draw on the window while recording. Recording uses
  /// ScreenCaptureKit, so macOS asks once to let the app record the screen; only the app's own
  /// windows end up in the movie. Narration needs `NSMicrophoneUsageDescription` in the app's
  /// Info.plist (sandboxed apps also need the audio-input entitlement).
  ///
  /// With `ScrawlCore` (`SCRAWL_CORE=1`) the bundle is built right here (`compiles = .onDevice`,
  /// the default then) and handed to `onBundle`, or shown in the Finder. Without it, the movie
  /// and its timeline are shown in the Finder for `scrawl import`. With a `receiver` (a
  /// `scrawl-server`), each bundle built here is sent there too, retried until it lands.
  @MainActor
  @Observable
  public final class Scrawl {
    public static let shared = Scrawl()

    public enum Compiling: Sendable {
      /// Left for Scrawl on this Mac: the movie and its timeline (`scrawl import <timeline>`).
      case onMac
      /// Built here with `ScrawlCore` (`SCRAWL_CORE=1`), the narration transcribed with Apple's
      /// on-device speech, then handed to `onBundle`. Transcribing needs
      /// `NSSpeechRecognitionUsageDescription` in the app's Info.plist.
      case onDevice
    }

    public struct Recording: Sendable {
      public let movie: URL
      public let timeline: URL
    }

    /// The key that starts and stops a recording, with its modifiers.
    public struct Shortcut: Equatable, Sendable {
      public var key: String
      public var modifiers: NSEvent.ModifierFlags

      public init(key: String, modifiers: NSEvent.ModifierFlags) {
        self.key = key.lowercased()
        self.modifiers = modifiers.intersection(.deviceIndependentFlagsMask)
      }

      /// ⌃⌥⇧S: Scrawl's own ⌥⇧S, with ⌃ so the two never collide.
      public static let `default` = Shortcut(key: "s", modifiers: [.control, .option, .shift])

      func matches(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
          .subtracting([.capsLock, .numericPad, .function])
        return flags == modifiers && event.charactersIgnoringModifiers?.lowercased() == key
      }

      var symbols: String {
        var out = ""
        if modifiers.contains(.control) { out += "⌃" }
        if modifiers.contains(.option) { out += "⌥" }
        if modifiers.contains(.shift) { out += "⇧" }
        if modifiers.contains(.command) { out += "⌘" }
        return out + key.uppercased()
      }
    }

    public private(set) var isRecording = false
    /// Asking ScreenCaptureKit to start; the overlay is up.
    public private(set) var isStarting = false
    /// ⇧ is held while recording: dragging draws instead of reaching the app.
    public private(set) var isDrawing = false
    /// A stopped recording is being saved or built.
    public private(set) var isFinishing = false
    /// The recordings made so far, newest last.
    public private(set) var recordings: [Recording] = []

    /// Whether to record the microphone with the window.
    public var narration = true
    /// Where recordings become bundles. Takes effect on the next recording.
    public var compiles: Compiling = Scrawl.defaultCompiling
    /// Gets each bundle built here (`compiles = .onDevice`): its `<id>.scrawl` folder in
    /// `directory`. Without it, the Finder shows it when `revealsWhenDone`.
    public var onBundle: (@MainActor (URL) -> Void)?
    /// Show what was saved in the Finder when a recording is done.
    public var revealsWhenDone = true
    /// Scrawl's experiments to try in bundles built here, by name (`"agentContext"`).
    public var experiments: Set<String> = []
    /// Starts and stops a recording while the app is in front.
    public var shortcut: Shortcut = .default
    /// Adds Record with Scrawl (and Mark, Discard) to the app's Help menu.
    public var addsMenuItems = true
    /// A Scrawl receiver (`scrawl-server`) each bundle built here is also sent to, with its upload
    /// token. Saved in the Keychain; Help › Send Recordings to a Scrawl Server sets it from a
    /// setup link on the clipboard. Nil: bundles stay on this Mac.
    public var receiver: Receiver? = Receiver.saved() {
      didSet {
        guard receiver != oldValue else { return }
        receiverIsSaved = Receiver.save(receiver)
        if installed { syncOutbox() }
      }
    }
    /// Whether the Keychain took the last change to `receiver`. False: the next launch starts
    /// with whatever the Keychain held before (another server, or none).
    public private(set) var receiverIsSaved = true

    /// Where recordings and bundles go: `Application Support/ScrawlKit/<bundle id>` (inside the
    /// app's container when it's sandboxed).
    public static var directory: URL {
      URL.applicationSupportDirectory
        .appending(path: "ScrawlKit", directoryHint: .isDirectory)
        .appending(path: Bundle.main.bundleIdentifier ?? "app", directoryHint: .isDirectory)
    }

    private static var defaultCompiling: Compiling {
      #if SCRAWL_CORE
        .onDevice
      #else
        .onMac
      #endif
    }

    @ObservationIgnored private var installed = false
    @ObservationIgnored private var monitors: [Any] = []
    @ObservationIgnored private let menuTarget = MenuTarget()
    /// Sends bundles to `receiver` and retries until each lands; exists only while there is one.
    @ObservationIgnored private var outbox: Outbox?
    @ObservationIgnored private var recorder: MacScreenRecorder?
    @ObservationIgnored private var overlay: MacOverlay?
    @ObservationIgnored private weak var window: NSWindow?
    @ObservationIgnored private var moveObserver: (any NSObjectProtocol)?
    /// The recorded area, in global AppKit points, and the display frame it's measured on.
    @ObservationIgnored private var area: CGRect = .zero
    @ObservationIgnored private var display: CGRect = .zero
    /// The area and its window's frame when recording started: where every move is measured
    /// from.
    @ObservationIgnored private var startArea: CGRect = .zero
    @ObservationIgnored private var startWindowFrame: CGRect = .zero
    /// The area the capture was last given, and the one it started with.
    @ObservationIgnored private var capturedArea: CGRect = .zero
    @ObservationIgnored private var startCapturedArea: CGRect = .zero
    @ObservationIgnored private var scale: CGFloat = 1
    /// Host time (seconds) of the movie's first frame: the timeline's zero.
    @ObservationIgnored private var zero: Double?
    @ObservationIgnored private var startedAt: Date?
    @ObservationIgnored private var markers: [Double] = []
    @ObservationIgnored private var events: [[String: Any]] = []
    @ObservationIgnored private var touches: [[String: Any]] = []
    @ObservationIgnored private var strokes: [[String: Any]] = []
    @ObservationIgnored private var stroke: [[Double]]?
    @ObservationIgnored private var touchID = 0
    @ObservationIgnored private var openTouch: Int?
    @ObservationIgnored private var lastMove: Double = 0
    /// Where the window was when the open touch went down: a drag that moves it is the title bar's.
    @ObservationIgnored private var touchWindowOrigin: CGPoint?

    // MARK: - Setup

    /// Turns on the shortcut and the Help menu items. `.scrawl()` calls it; an AppKit app calls
    /// it once, after launch. Calling it again is harmless.
    public func install() {
      addMenuItems()
      guard !installed else { return }
      installed = true
      syncOutbox()
      monitors.append(NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
        let passes = MainActor.assumeIsolated { Scrawl.shared.handleKey(event) }
        return passes ? event : nil
      } as Any)
      monitors.append(NSEvent.addLocalMonitorForEvents(
        matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp, .flagsChanged]
      ) { event in
        let passes = MainActor.assumeIsolated { Scrawl.shared.handleMouse(event) }
        return passes ? event : nil
      } as Any)
      // SwiftUI can build the menu bar after the first view appears, and rebuild it.
      _ = NotificationCenter.default.addObserver(
        forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
      ) { _ in
        MainActor.assumeIsolated { Scrawl.shared.addMenuItems() }
      }
      DispatchQueue.main.async { Scrawl.shared.addMenuItems() }
    }

    /// Logs something the app did ("Saved", "sync finished"), placed on the recording's timeline.
    public static func event(_ label: String) {
      MainActor.assumeIsolated { shared.record(event: label) }
    }

    // MARK: - Recording

    /// Starts recording, or stops and sends the current recording.
    public func toggle() {
      isRecording || isStarting ? stop() : start()
    }

    /// Starts recording the app's window in front.
    public func start() {
      guard !isRecording, !isStarting, !isFinishing else { return }
      guard let window = Self.windowToRecord(), let screen = window.screen else {
        print("Scrawl: no window to record")
        return
      }
      let area = MacGeometry.area(of: window.frame, on: screen.frame)
      guard area.width >= 2, area.height >= 2 else { return }
      let displayID = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)
        .map { CGDirectDisplayID($0.uint32Value) } ?? CGMainDisplayID()
      self.window = window
      self.area = area
      startArea = area
      startWindowFrame = window.frame
      capturedArea = .zero
      display = screen.frame
      scale = screen.backingScaleFactor
      zero = nil
      startedAt = nil
      markers = []
      events = []
      touches = []
      strokes = []
      stroke = nil
      touchID = 0
      openTouch = nil
      isStarting = true

      let overlay = overlay ?? MacOverlay(over: area)
      self.overlay = overlay
      overlay.move(to: area)
      overlay.clearInk()
      overlay.show("Starting…")

      try? FileManager.default.createDirectory(at: Self.directory, withIntermediateDirectories: true)
      let movie = Self.directory.appending(path: "\(Self.stamp(Date())).mp4")
      let recorder = MacScreenRecorder(movie: movie)
      recorder.onStop = { [weak recorder] error in
        Task { @MainActor in
          if let recorder { Scrawl.shared.recorderStopped(recorder, error) }
        }
      }
      // Set before it starts: the first frame (and a stop right after it) can come before
      // `start` returns.
      self.recorder = recorder
      moveObserver = NotificationCenter.default.addObserver(
        forName: NSWindow.didMoveNotification, object: window, queue: .main
      ) { _ in
        MainActor.assumeIsolated { Scrawl.shared.windowMoved() }
      }
      let overlayID = overlay.windowID
      let scale = scale
      Task {
        var narrated = false
        if narration { narrated = await Self.microphoneAllowed() }
        // Stopped while the microphone prompt was up.
        guard self.recorder === recorder else { return }
        // The window may have moved while the microphone prompt was up.
        windowMoved()
        capturedArea = self.area
        startCapturedArea = self.area
        let source = MacGeometry.sourceRect(of: self.area, on: display)
        // Nothing in a few seconds (a window that never draws, a stuck capture): give up.
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) {
          MainActor.assumeIsolated {
            let scrawl = Scrawl.shared
            guard scrawl.isStarting, scrawl.recorder === recorder else { return }
            scrawl.cancelStart(showing: "Recording didn't start: no frame came from the window")
          }
        }
        do {
          try await recorder.start(
            area: .init(displayID: displayID, sourceRect: source, scale: scale),
            leavingOut: [overlayID],
            narration: narrated
          ) { host in
            Task { @MainActor in Scrawl.shared.began(recorder, at: host) }
          }
        } catch {
          guard self.recorder === recorder else { return }
          print("Scrawl: recording didn't start: \(error.localizedDescription)")
          cancelStart(showing: Self.startFailure(error))
        }
      }
    }

    /// Stops recording and sends it (builds it, or leaves it for Scrawl on this Mac).
    public func stop() {
      finish(discarding: false)
    }

    /// Stops recording and throws it away.
    public func discard() {
      finish(discarding: true)
    }

    /// Marks this moment ("look here").
    public func mark() {
      guard isRecording, let zero else { return }
      markers.append(Self.round(CACurrentMediaTime() - zero))
      NSHapticFeedbackManager.defaultPerformer.perform(.generic, performanceTime: .now)
    }

    private func began(_ recorder: MacScreenRecorder, at host: Double) {
      guard isStarting, self.recorder === recorder else { return }
      zero = host
      let startedAt = Date().addingTimeInterval(-(CACurrentMediaTime() - host))
      self.startedAt = startedAt
      isStarting = false
      isRecording = true
      ScrawlAppContext.shared.begin(at: startedAt)
      overlay?.show("● Recording · \(shortcut.symbols) sends · hold ⇧ to draw")
      // A move made before the stream existed was dropped: send the area again if it moved.
      if area != startCapturedArea {
        capturedArea = area
        recorder.move(to: MacGeometry.sourceRect(of: area, on: display))
      }
    }

    /// Keeps the recorded area, the overlay and (once it has an area) the capture on the window.
    private func windowMoved() {
      guard isRecording || isStarting, let window else { return }
      area = MacGeometry.moved(startArea, from: startWindowFrame, to: window.frame, on: display)
      overlay?.move(to: area)
      // `capturedArea` is zero until the capture is handed its area, after the microphone prompt.
      if capturedArea != .zero, area != capturedArea {
        capturedArea = area
        recorder?.move(to: MacGeometry.sourceRect(of: area, on: display))
      }
    }

    /// Ends a recording that hasn't shown a frame yet: there is nothing to save.
    private func cancelStart(showing message: String?) {
      guard isStarting, let recorder else { return }
      isStarting = false
      self.recorder = nil
      stopFollowingWindow()
      recorder.cancel()
      if let message {
        showBriefly(message)
      } else {
        overlay?.orderOut(nil)
      }
    }

    private func stopFollowingWindow() {
      if let moveObserver { NotificationCenter.default.removeObserver(moveObserver) }
      moveObserver = nil
    }

    private func recorderStopped(_ recorder: MacScreenRecorder, _ error: any Error) {
      guard self.recorder === recorder, isRecording || isStarting else { return }
      print("Scrawl: recording stopped: \(error.localizedDescription)")
      guard isRecording else {
        cancelStart(showing: Self.startFailure(error))
        return
      }
      finish(discarding: false)
    }

    private func finish(discarding: Bool) {
      if isStarting {
        // No frame yet, so nothing to send either way.
        cancelStart(showing: discarding ? nil : "Stopped before anything was recorded")
        return
      }
      guard isRecording, let recorder, let zero, let startedAt else { return }
      isRecording = false
      isDrawing = false
      isFinishing = true
      self.recorder = nil
      stopFollowingWindow()
      endStroke(at: CACurrentMediaTime() - zero)
      // A second press straight after the first (nothing done, drawn or marked in it) is a slip,
      // not a recording to build.
      let tooShort = !discarding && CACurrentMediaTime() - zero < 1
        && touches.isEmpty && strokes.isEmpty && markers.isEmpty && events.isEmpty
      let throwAway = discarding || tooShort

      let bundleInfo = Bundle.main.infoDictionary ?? [:]
      let version = ProcessInfo.processInfo.operatingSystemVersion
      var info: [String: Any] = [
        "app": bundleInfo["CFBundleDisplayName"] as? String
          ?? bundleInfo["CFBundleName"] as? String
          ?? ProcessInfo.processInfo.processName,
        "bundleId": Bundle.main.bundleIdentifier ?? "",
        "version": bundleInfo["CFBundleVersion"] as? String ?? "",
        "shortVersion": bundleInfo["CFBundleShortVersionString"] as? String ?? "",
        "device": "Mac",
        "system": "macOS \(version.majorVersion).\(version.minorVersion)",
        // Clicks and ink are in these points, from the recorded area's top-left corner.
        "screen": ["width": area.width, "height": area.height, "scale": scale],
        "recordedAt": startedAt.ISO8601Format(),
        "duration": Self.round(CACurrentMediaTime() - zero),
        "markers": markers,
        "events": events,
        "touches": touches,
        "strokes": strokes,
        "capture": "screenCaptureKit",
      ]
      info.merge(ScrawlAppContext.shared.end()) { current, _ in current }
      self.zero = nil
      self.startedAt = nil
      overlay?.clearInk()
      if tooShort {
        showBriefly("Too short to keep")
      } else {
        overlay?.show(throwAway ? nil : "Saving…")
        if throwAway { overlay?.orderOut(nil) }
      }

      Task {
        defer { isFinishing = false }
        do {
          try await recorder.stop()
        } catch {
          print("Scrawl: the recording couldn't be saved: \(error.localizedDescription)")
          try? FileManager.default.removeItem(at: recorder.movie)
          showBriefly("The recording couldn't be saved")
          return
        }
        if throwAway {
          try? FileManager.default.removeItem(at: recorder.movie)
          return
        }
        info["audio"] = recorder.recordsAudio
        let timeline = recorder.movie.deletingPathExtension().appendingPathExtension("json")
        let data = (try? JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys])) ?? Data()
        do {
          try data.write(to: timeline)
        } catch {
          print("Scrawl: the timeline couldn't be saved: \(error.localizedDescription)")
          // Building here uses `data`; anything else needs the file.
          if !buildsHere {
            if revealsWhenDone { NSWorkspace.shared.activateFileViewerSelecting([recorder.movie]) }
            showBriefly("The timeline couldn't be saved; the movie is kept")
            return
          }
        }
        await deliver(Recording(movie: recorder.movie, timeline: timeline), timeline: data, createdAt: startedAt)
      }
    }

    /// Whether a finished recording is built into a bundle in this app.
    private var buildsHere: Bool {
      #if SCRAWL_CORE
        compiles == .onDevice
      #else
        false
      #endif
    }

    private func deliver(_ recording: Recording, timeline: Data, createdAt: Date) async {
      #if SCRAWL_CORE
        if compiles == .onDevice {
          overlay?.show("Building the bundle…")
          let narrated = ((try? JSONSerialization.jsonObject(with: timeline)) as? [String: Any])?["audio"] as? Bool ?? false
          let id = recording.movie.deletingPathExtension().lastPathComponent
          let directory = Self.directory
          let experiments = experiments
          let built = await Task.detached(priority: .userInitiated) { () -> Result<URL, any Error> in
            let transcript = await OnDeviceSpeech.transcript(of: recording.movie, narrated: narrated)
            do {
              return try await .success(OnDeviceBuild.build(
                movie: recording.movie, timeline: timeline, id: id, createdAt: createdAt,
                transcript: transcript, experiments: experiments, into: directory
              ))
            } catch {
              return .failure(error)
            }
          }.value
          switch built {
          case let .success(bundle):
            // The bundle holds its own copies now.
            for file in [recording.movie, recording.timeline] {
              try? FileManager.default.removeItem(at: file)
            }
            recordings.append(Recording(
              movie: bundle.appending(path: "recording.\(recording.movie.pathExtension.lowercased())"),
              timeline: bundle.appending(path: "timeline.json")
            ))
            if let onBundle {
              onBundle(bundle)
            } else if revealsWhenDone {
              NSWorkspace.shared.activateFileViewerSelecting([bundle])
            }
            await send(bundle, createdAt: createdAt)
          case let .failure(error):
            print("Scrawl: building the bundle failed: \(error)")
            recordings.append(recording)
            if revealsWhenDone {
              NSWorkspace.shared.activateFileViewerSelecting([recording.movie, recording.timeline])
            }
            showBriefly("The bundle couldn't be built; the recording is saved")
          }
          return
        }
      #endif
      recordings.append(recording)
      if revealsWhenDone {
        NSWorkspace.shared.activateFileViewerSelecting([recording.movie, recording.timeline])
      }
      showBriefly("Saved the recording")
    }

    // MARK: - Receiver

    /// Creates the outbox while there's a receiver (sending what was waiting from earlier
    /// launches) and stops it when there isn't.
    private func syncOutbox() {
      guard let receiver else {
        outbox?.invalidate()
        outbox = nil
        return
      }
      let outbox = outbox ?? Outbox(
        directory: Self.directory.appending(path: "Outbox", directoryHint: .isDirectory),
        configuration: .default
      ) { event in
        Task { @MainActor in Scrawl.shared.handle(event) }
      }
      self.outbox = outbox
      outbox.setCredentials(target: receiver.url, token: receiver.token)
      // A new receiver or token: what it refused before gets another go.
      outbox.retryFailed()
    }

    /// Queues `bundle` for the receiver, if there is one, and says so.
    private func send(_ bundle: URL, createdAt: Date) async {
      guard let receiver, let outbox else {
        showBriefly("Saved \(bundle.lastPathComponent)")
        return
      }
      do {
        let entry = try await outbox.enqueue(
          bundle: bundle, target: receiver.url, recordedAt: createdAt.ISO8601Format()
        )
        if let failure = entry.failure {
          handle(.failed(id: entry.id, failure))
          return
        }
        // The receiver may have been removed (and its outbox stopped) while the bundle was zipped;
        // the entry stays on disk for the next one.
        guard let current = self.outbox else {
          showBriefly("Saved \(bundle.lastPathComponent)")
          return
        }
        current.kick(force: true)
        showBriefly("Sending to your Scrawl server…")
      } catch {
        print("Scrawl: the bundle couldn't be queued for the Scrawl server: \(error)")
        showBriefly("Saved \(bundle.lastPathComponent); it couldn't be queued for your Scrawl server")
      }
    }

    private func handle(_ event: Outbox.Event) {
      switch event {
      case .sent:
        showBriefly("Sent to your Scrawl server")
      case let .failed(_, failure):
        switch failure {
        case .unauthorized: showBriefly("Your Scrawl server refused the token; the bundle is kept")
        case .tooLarge: showBriefly("Too large for your Scrawl server; the bundle is kept")
        case .rejected: showBriefly("Your Scrawl server refused the bundle; it's kept")
        }
      case .started, .progress, .retrying:
        break
      }
    }

    /// Help › Send Recordings to a Scrawl Server: takes a setup link from the clipboard, or
    /// forgets the receiver.
    fileprivate func toggleReceiver() {
      if let receiver {
        self.receiver = nil
        if receiverIsSaved {
          alert("Recordings stay on this Mac", detail: "They're no longer sent to \(receiver.url.host() ?? "your Scrawl server").")
        } else {
          alert(
            "Stopped sending for now",
            detail: "The Keychain wouldn't forget \(receiver.url.host() ?? "the server"), so sending may start again next time the app opens. Remove “dev.kitlangton.scrawl.receiver” in Keychain Access to stop it for good."
          )
        }
        return
      }
      guard let link = NSPasteboard.general.string(forType: .string), let found = Receiver(setupLink: link) else {
        alert(
          "Copy a Scrawl server setup link first",
          detail: "It looks like app://scrawl?url=https%3A%2F%2F…&token=…, made with your server's upload token (an https address)."
        )
        return
      }
      let confirm = NSAlert()
      confirm.messageText = "Send recordings to \(found.url.host() ?? "this Scrawl server")?"
      confirm.informativeText = "Each bundle built here is sent there too, until you choose Stop Sending in the Help menu."
      confirm.addButton(withTitle: "Send")
      confirm.addButton(withTitle: "Cancel")
      guard confirm.runModal() == .alertFirstButtonReturn else { return }
      receiver = found
      if !receiverIsSaved {
        alert(
          "Sending until the app quits",
          detail: "The Keychain wouldn't keep the server. Next time the app opens, check the Help menu and choose it again."
        )
      }
    }

    private func alert(_ text: String, detail: String) {
      let alert = NSAlert()
      alert.messageText = text
      alert.informativeText = detail
      alert.runModal()
    }

    fileprivate var receiverMenuTitle: String {
      receiver.map { "Stop Sending to \($0.url.host() ?? "the Scrawl Server")" } ?? "Send Recordings to a Scrawl Server"
    }

    /// Shows `text` on the overlay for a moment, then hides it.
    private func showBriefly(_ text: String) {
      guard let overlay else { return }
      overlay.show(text)
      DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
        MainActor.assumeIsolated {
          let scrawl = Scrawl.shared
          guard !scrawl.isRecording, !scrawl.isStarting else { return }
          scrawl.overlay?.orderOut(nil)
        }
      }
    }

    // MARK: - Input

    /// Whether the app gets `event` (false for the shortcut).
    private func handleKey(_ event: NSEvent) -> Bool {
      guard shortcut.matches(event) else { return true }
      toggle()
      return false
    }

    /// Whether the app gets `event` (false for the drags that draw ink).
    private func handleMouse(_ event: NSEvent) -> Bool {
      guard isRecording, let zero, let overlay, event.window !== overlay else { return true }
      if event.type == .flagsChanged {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
          .subtracting([.capsLock, .numericPad, .function])
        let drawing = flags == .shift
        if drawing != isDrawing {
          isDrawing = drawing
          if !drawing { endStroke(at: event.timestamp - zero) }
        }
        return true
      }
      let global = event.window.map { $0.convertPoint(toScreen: event.locationInWindow) } ?? NSEvent.mouseLocation
      let point = MacGeometry.point(global, in: area)
      let t = Self.round(event.timestamp - zero)
      let sample = [Self.round(point.x), Self.round(point.y), t]
      switch event.type {
      case .leftMouseDown:
        if isDrawing {
          stroke = [sample]
          overlay.beginStroke(at: point)
          return false
        }
        touchID += 1
        openTouch = touchID
        touchWindowOrigin = window?.frame.origin
        lastMove = t
        var touch: [String: Any] = ["t": t, "id": touchID, "x": sample[0], "y": sample[1], "phase": "down"]
        if let label = Self.label(at: global) { touch["label"] = label }
        touches.append(touch)
      case .leftMouseDragged:
        if stroke != nil {
          stroke?.append(sample)
          overlay.extendStroke(to: point)
          return false
        }
        guard let id = openTouch else { return true }
        if dropTouchIfItMovedTheWindow(id) { return true }
        // Moves at most 30 a second, as on the phone.
        guard t - lastMove >= 1.0 / 30 else { return true }
        lastMove = t
        touches.append(["t": t, "id": id, "x": sample[0], "y": sample[1], "phase": "move"])
      case .leftMouseUp:
        if stroke != nil {
          stroke?.append(sample)
          endStroke(at: t)
          return false
        }
        guard let id = openTouch else { return true }
        if dropTouchIfItMovedTheWindow(id) { return true }
        openTouch = nil
        touches.append(["t": t, "id": id, "x": sample[0], "y": sample[1], "phase": "up"])
      default:
        break
      }
      return true
    }

    /// A drag on the title bar moves the window, which the recording follows: it isn't something
    /// done in the app, and drawn as a drag it would cross the app's content.
    private func dropTouchIfItMovedTheWindow(_ id: Int) -> Bool {
      guard let origin = touchWindowOrigin, let window, window.frame.origin != origin else { return false }
      touches.removeAll { $0["id"] as? Int == id }
      openTouch = nil
      return true
    }

    private func endStroke(at t: Double) {
      guard let points = stroke, let first = points.first else { return }
      stroke = nil
      strokes.append(["t0": first[2], "t1": Self.round(max(t, points.last?[2] ?? first[2])), "points": points])
      overlay?.endStroke()
    }

    private func record(event label: String) {
      guard isRecording, let zero else { return }
      events.append(["t": Self.round(CACurrentMediaTime() - zero), "label": label])
    }

    /// What's under a click, from the app's accessibility: the nearest label, title or value.
    private static func label(at global: CGPoint) -> String? {
      var element = NSApp.accessibilityHitTest(global) as? NSObject
      for _ in 0 ..< 6 {
        guard let current = element else { return nil }
        for name in ["accessibilityLabel", "accessibilityTitle", "accessibilityValue"] {
          if let text = accessibility(name, of: current) as? String {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty { return String(trimmed.prefix(80)) }
          }
        }
        element = accessibility("accessibilityParent", of: current) as? NSObject
      }
      return nil
    }

    /// One accessibility property of `element`, when it has it. AppKit's own elements and
    /// SwiftUI's answer these; asking by name works for both.
    private static func accessibility(_ name: String, of element: NSObject) -> Any? {
      let selector = NSSelectorFromString(name)
      guard element.responds(to: selector) else { return nil }
      return element.perform(selector)?.takeUnretainedValue()
    }

    // MARK: - Menu

    private static let menuTag = 0x5C4A
    fileprivate var menuTitle: String { isRecording || isStarting ? "Stop and Send Scrawl Recording" : "Record with Scrawl" }

    private func addMenuItems() {
      guard addsMenuItems, let menu = Self.helpMenu() else { return }
      guard !menu.items.contains(where: { $0.tag == Self.menuTag }) else { return }
      let separator = NSMenuItem.separator()
      separator.tag = Self.menuTag
      let record = NSMenuItem(title: menuTitle, action: #selector(MenuTarget.toggle(_:)), keyEquivalent: shortcut.key)
      record.keyEquivalentModifierMask = shortcut.modifiers
      let mark = NSMenuItem(title: "Mark This Moment", action: #selector(MenuTarget.mark(_:)), keyEquivalent: "")
      let discard = NSMenuItem(title: "Discard Scrawl Recording", action: #selector(MenuTarget.discard(_:)), keyEquivalent: "")
      let receiver = NSMenuItem(title: receiverMenuTitle, action: #selector(MenuTarget.receiver(_:)), keyEquivalent: "")
      for item in [separator, record, mark, discard, receiver] {
        item.tag = Self.menuTag
        item.target = menuTarget
        menu.addItem(item)
      }
    }

    private static func helpMenu() -> NSMenu? {
      if let menu = NSApp.helpMenu { return menu }
      guard let main = NSApp.mainMenu else { return nil }
      if let item = main.items.first(where: { $0.title == "Help" }), let menu = item.submenu {
        return menu
      }
      let menu = NSMenu(title: "Help")
      let item = NSMenuItem(title: "Help", action: nil, keyEquivalent: "")
      item.submenu = menu
      main.addItem(item)
      NSApp.helpMenu = menu
      return menu
    }

    // MARK: - Helpers

    /// The app's window in front, not Scrawl's overlay.
    private static func windowToRecord() -> NSWindow? {
      let candidates = [NSApp.keyWindow, NSApp.mainWindow] + NSApp.orderedWindows.map(Optional.some)
      return candidates.compactMap { $0 }.first { window in
        window.isVisible && !(window is MacOverlay) && !(window is NSPanel && !window.canBecomeMain)
      }
    }

    /// Whether narration can be recorded, asking the first time.
    private static func microphoneAllowed() async -> Bool {
      guard Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") != nil else {
        print("Scrawl: add NSMicrophoneUsageDescription to the app's Info.plist to record narration")
        return false
      }
      switch AVCaptureDevice.authorizationStatus(for: .audio) {
      case .authorized: return true
      case .notDetermined: return await AVCaptureDevice.requestAccess(for: .audio)
      default: return false
      }
    }

    private static func startFailure(_ error: any Error) -> String {
      let text = "\(error)".lowercased()
      if text.contains("declined") || text.contains("permission") || text.contains("tcc") {
        return "Allow screen recording for this app in System Settings › Privacy & Security"
      }
      return "Recording didn't start: \(error.localizedDescription)"
    }

    private static func stamp(_ date: Date) -> String {
      date.formatted(.iso8601.year().month().day().timeSeparator(.omitted).time(includingFractionalSeconds: false))
        .replacingOccurrences(of: ":", with: "")
    }

    private static func round(_ value: Double) -> Double { (value * 1000).rounded() / 1000 }
  }

  /// The Help menu items' target (menu actions need an Objective-C object).
  private final class MenuTarget: NSObject, NSMenuItemValidation {
    @objc func toggle(_: Any?) {
      MainActor.assumeIsolated { Scrawl.shared.toggle() }
    }

    @objc func mark(_: Any?) {
      MainActor.assumeIsolated { Scrawl.shared.mark() }
    }

    @objc func discard(_: Any?) {
      MainActor.assumeIsolated { Scrawl.shared.discard() }
    }

    @objc func receiver(_: Any?) {
      MainActor.assumeIsolated { Scrawl.shared.toggleReceiver() }
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
      MainActor.assumeIsolated {
        let scrawl = Scrawl.shared
        switch item.action {
        case #selector(toggle(_:)):
          item.title = scrawl.menuTitle
          return !scrawl.isFinishing
        case #selector(mark(_:)), #selector(discard(_:)):
          return scrawl.isRecording
        case #selector(receiver(_:)):
          item.title = scrawl.receiverMenuTitle
          return !scrawl.isRecording && !scrawl.isStarting
        default:
          return true
        }
      }
    }
  }

  public extension View {
    /// Turns Scrawl on for this app: the shortcut (⌃⌥⇧S) and Help › Record with Scrawl.
    func scrawl() -> some View {
      onAppear { Scrawl.shared.install() }
    }
  }
#endif
