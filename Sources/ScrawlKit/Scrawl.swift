#if os(iOS)
  import Network
  import Security
  import SwiftUI
  import UIKit

  /// Per-device Scrawl ingest configuration, stored in the Keychain under `dev.kitlangton.scrawl`.
  public struct ScrawlConfig: Codable, Equatable, Sendable {
    public static let keychainService = "dev.kitlangton.scrawl"
    private static let legacyKeychainService = "dev.kitlangton.tether"
    private static let keychainAccount = "config"
    private static let setupHosts: Set<String> = ["scrawl", "tether"]

    public var urls: [URL]
    public var token: String
    public var sessionID: String?

    public init(urls: [URL], token: String, sessionID: String? = nil) {
      self.urls = Self.validBaseURLs(urls)
      self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
      let trimmedSession = sessionID?.trimmingCharacters(in: .whitespacesAndNewlines)
      self.sessionID = (trimmedSession?.isEmpty == false) ? trimmedSession : nil
    }

    public var isValid: Bool {
      !urls.isEmpty && !token.isEmpty
    }

    enum SetupAction: Equatable {
      case configure(ScrawlConfig)
      case reset
    }

    /// Parses `<scheme>://scrawl?url=<base>[&url=<second>]&token=<token>[&session=ses_…]`
    /// or `<scheme>://scrawl?reset=1` (also accepts the legacy `tether` host).
    /// Only `http://` and `https://` base URLs are accepted.
    static func parseSetupURL(_ url: URL) -> SetupAction? {
      guard let scheme = url.scheme, !scheme.isEmpty,
            let host = url.host?.lowercased(),
            setupHosts.contains(host),
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
      else { return nil }

      if items.contains(where: {
        $0.name == "reset" && ($0.value == "1" || $0.value?.lowercased() == "true")
      }) {
        return .reset
      }

      let rawURLs = items
        .filter { $0.name == "url" }
        .compactMap(\.value)
        .compactMap(Self.parseHTTPBaseURL)
      let urls = Self.validBaseURLs(rawURLs)
      let token = items
        .first(where: { $0.name == "token" })?
        .value?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
      let session = items
        .first(where: { $0.name == "session" })?
        .value?
        .trimmingCharacters(in: .whitespacesAndNewlines)
      let config = ScrawlConfig(urls: urls, token: token, sessionID: session)
      guard config.isValid else { return nil }
      return .configure(config)
    }

    static func parseHTTPBaseURL(_ raw: String) -> URL? {
      var trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
      while trimmed.hasSuffix("/") {
        trimmed.removeLast()
      }
      guard !trimmed.isEmpty,
            let url = URL(string: trimmed),
            let scheme = url.scheme?.lowercased(),
            scheme == "http" || scheme == "https",
            let host = url.host,
            !host.isEmpty
      else { return nil }
      return url
    }

    static func validBaseURLs(_ candidates: [URL]) -> [URL] {
      var seen = Set<String>()
      var result: [URL] = []
      for candidate in candidates {
        guard let normalized = parseHTTPBaseURL(candidate.absoluteString),
              seen.insert(normalized.absoluteString).inserted
        else { continue }
        result.append(normalized)
      }
      return result
    }

    public static func loadFromKeychain() -> ScrawlConfig? {
      if let current = loadFromKeychain(service: keychainService) {
        return current
      }
      if let legacy = loadFromKeychain(service: legacyKeychainService) {
        _ = saveToKeychain(legacy)
        deleteFromKeychain(service: legacyKeychainService)
        return legacy
      }
      return nil
    }

    private static func loadFromKeychain(service: String) -> ScrawlConfig? {
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: keychainAccount,
        kSecReturnData as String: true,
        kSecMatchLimit as String: kSecMatchLimitOne,
      ]
      var result: CFTypeRef?
      let status = SecItemCopyMatching(query as CFDictionary, &result)
      guard status == errSecSuccess,
            let data = result as? Data,
            let decoded = try? JSONDecoder().decode(ScrawlConfig.self, from: data)
      else { return nil }
      let normalized = ScrawlConfig(urls: decoded.urls, token: decoded.token, sessionID: decoded.sessionID)
      return normalized.isValid ? normalized : nil
    }

    @discardableResult
    public static func saveToKeychain(_ config: ScrawlConfig) -> Bool {
      let normalized = ScrawlConfig(urls: config.urls, token: config.token, sessionID: config.sessionID)
      guard normalized.isValid, let data = try? JSONEncoder().encode(normalized) else { return false }
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: keychainService,
        kSecAttrAccount as String: keychainAccount,
      ]
      let updateStatus = SecItemUpdate(
        query as CFDictionary,
        [kSecValueData as String: data] as CFDictionary
      )
      if updateStatus == errSecSuccess { return true }
      var item = query
      item[kSecValueData as String] = data
      item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
      return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    public static func deleteFromKeychain() {
      deleteFromKeychain(service: keychainService)
      deleteFromKeychain(service: legacyKeychainService)
    }

    private static func deleteFromKeychain(service: String) {
      let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: keychainAccount,
      ]
      _ = SecItemDelete(query as CFDictionary)
    }

    /// Resolves the active configuration by overlaying launch arguments / environment variables
    /// (`-scrawl-url`, `SCRAWL_URL`, `-scrawl-token`, `SCRAWL_TOKEN`, `-scrawl-session`,
    /// `SCRAWL_SESSION`) on top of the Keychain configuration.
    static func resolved(
      arguments: [String] = ProcessInfo.processInfo.arguments,
      environment: [String: String] = ProcessInfo.processInfo.environment,
      keychain: ScrawlConfig? = loadFromKeychain()
    ) -> ScrawlConfig? {
      var overrideURLStrings: [String] = []
      var index = 0
      while index < arguments.count {
        if arguments[index] == "-scrawl-url", index + 1 < arguments.count {
          overrideURLStrings.append(contentsOf: arguments[index + 1].split(separator: ",").map(String.init))
          index += 2
        } else {
          index += 1
        }
      }
      if let envURL = environment["SCRAWL_URL"], !envURL.isEmpty {
        overrideURLStrings.append(contentsOf: envURL.split(separator: ",").map(String.init))
      }
      let overrideURLs = validBaseURLs(overrideURLStrings.compactMap(parseHTTPBaseURL))
      let urls = !overrideURLs.isEmpty ? overrideURLs : (keychain?.urls ?? [])

      var overrideToken: String?
      if let idx = arguments.firstIndex(of: "-scrawl-token"), idx + 1 < arguments.count {
        overrideToken = arguments[idx + 1].trimmingCharacters(in: .whitespacesAndNewlines)
      } else if let envToken = environment["SCRAWL_TOKEN"]?.trimmingCharacters(in: .whitespacesAndNewlines),
                !envToken.isEmpty
      {
        overrideToken = envToken
      }
      let token = (overrideToken?.isEmpty == false ? overrideToken : nil) ?? keychain?.token ?? ""

      var overrideSession: String?
      if let idx = arguments.firstIndex(of: "-scrawl-session"), idx + 1 < arguments.count {
        overrideSession = arguments[idx + 1].trimmingCharacters(in: .whitespacesAndNewlines)
      } else if let envSession = environment["SCRAWL_SESSION"]?.trimmingCharacters(in: .whitespacesAndNewlines),
                !envSession.isEmpty
      {
        overrideSession = envSession
      }
      let sessionID = (overrideSession?.isEmpty == false ? overrideSession : nil) ?? keychain?.sessionID

      let config = ScrawlConfig(urls: urls, token: token, sessionID: sessionID)
      return config.isValid ? config : nil
    }
  }

  /// Records your app's screen with narration, the touches that drove it, markers, freehand red
  /// strokes, and app events, all on one clock, and sends the result straight to Scrawl on your Mac
  /// (falling back to the share sheet if your Mac isn't reachable).
  ///
  ///     WindowGroup { ContentView().scrawl() }      // three-finger long press starts and stops
  ///     Scrawl.event("Done tapped")                 // optional: what the app did, on the timeline
  ///
  /// Scrawl is completely inert until configured on the device via a `<scheme>://scrawl?...` setup
  /// link (generated by `scrawl link --scheme <scheme>`) or debug launch arguments.
  ///
  /// While recording, a slim strip on the screen's edge (drag its dot to move it) toggles drawing
  /// red strokes, discards (with Undo), or sends to your Mac / agent. The trigger again also sends.
  /// Every touch goes on the timeline with the label of the element it hit; nothing is drawn for
  /// it on screen (Scrawl on the Mac draws touches from the timeline). Holding still gives a tick.
  @MainActor
  @Observable
  public final class Scrawl {
    public static let shared = Scrawl()
    static let configurationDidChangeNotification = Notification.Name("dev.kitlangton.scrawl.configurationDidChange")

    public enum UploadStatus: Equatable, Sendable {
      case idle
      case sending
      /// The server took it; what it said became of it (delivered, or on the Mac but not yet).
      case sent(IngestReply)
      /// Saved on the phone and sending in the background (HTTPS endpoints): offline, or waiting
      /// to retry after a failed attempt.
      case queued
      case failed
    }

    public private(set) var isRecording = false
    public private(set) var isDrawing = false
    public private(set) var uploadStatus: UploadStatus = .idle
    /// Why the last delivery failed: no Mac answered (LAN; it waits for USB pickup or Share), the
    /// HTTPS endpoint answered 401/403 (link revoked) or 413, or it couldn't be saved for sending.
    private(set) var deliveryFailure = ScrawlStrip.Failure.unreachable
    /// A short message beside the strip while recording (link expired, the length cap).
    private(set) var notice: ScrawlStrip.Notice?
    /// How much of the shown HTTPS upload has gone out (0…1), once it reports bytes.
    private(set) var sendProgress: Double?
    /// The shown HTTPS upload is running on a working network but hasn't moved in a while.
    private(set) var isSendStalled = false
    /// A discarded recording is waiting out its Undo window.
    private(set) var isDiscardPending = false
    public private(set) var startedAt: Date?
    public private(set) var markers: [TimeInterval] = []
    /// The strokes drawn in this recording so far (smoothed), whether or not they've faded from
    /// the screen.
    public private(set) var completedStrokes: [[CGPoint]] = []
    /// Always empty: the stroke being drawn lives in the overlay's ink layer.
    @available(*, deprecated, message: "The stroke being drawn isn't published any more.")
    public private(set) var activeStroke: [CGPoint] = []
    /// The recordings made so far, newest last.
    public private(set) var recordings: [Recording] = []

    public struct Recording: Sendable {
      public let movie: URL
      public let timeline: URL
    }

    /// A stopped recording written to disk, ready to deliver (or delete, once discarded).
    private struct FinishedRecording {
      let recording: Recording
      let timelineData: Data
      let readyMarker: URL
      let sentMarker: URL
      let recordedAt: String
    }

    /// Recordings for HTTPS endpoints must stay under the 95 MB upload limit: app-frame video at
    /// `AppFrameRecorder.hostedVideoBitRate` is about 0.39 MB a second, so 3 minutes is about
    /// 70 MB. ReplayKit's bitrate can't be set (native resolution, an estimated 8–12 Mbps), so its
    /// recordings stop sooner. The strip warns `limitWarning` before the cap, then sends.
    static func hostedRecordingLimit(for method: ScrawlCaptureMethod) -> Duration {
      switch method {
      case .appFrames: .seconds(180)
      case .replayKit: .seconds(45)
      }
    }

    static let limitWarning: Duration = .seconds(10)

    private static let undoWindow: Duration = .seconds(5)
    @ObservationIgnored private var events: [[String: Any]] = []
    @ObservationIgnored private var touchLog: [[String: Any]] = []
    @ObservationIgnored private var strokeLog: [[String: Any]] = []
    @ObservationIgnored private var lastTouchLog: [Int: TimeInterval] = [:]
    @ObservationIgnored private var touchLabels: [Int: String] = [:]
    /// Small sequential IDs per finger (1, 2, 3…), stable for a touch's lifetime.
    @ObservationIgnored private var touchIDs: [ObjectIdentifier: Int] = [:]
    @ObservationIgnored private var nextTouchID = 1
    /// The scroll view under each touch as it went down, where its content was then, and where
    /// the finger was: to tell a swipe that moved nothing (`settleSwipe`).
    @ObservationIgnored private var swipeStarts: [Int: SwipeStart] = [:]
    /// Touches that may become a press-and-hold, by ID: where each went down.
    @ObservationIgnored private var holdStarts: [Int: CGPoint] = [:]
    /// Touches currently moving across the screen, plus the scroll views they touched.
    @ObservationIgnored private var draggingTouchIDs: Set<Int> = []
    @ObservationIgnored private var trackedScrollViews: [WeakScrollView] = []
    @ObservationIgnored private(set) var lastTouchMoveMediaTime: TimeInterval?
    /// The window the touches are measured in, for the timeline's `screen`.
    @ObservationIgnored weak var touchWindow: UIWindow?
    @ObservationIgnored private var overlay: OverlayWindow?
    @ObservationIgnored private var prewarmExpiry: Task<Void, Never>?
    private(set) var preparedAt: Date?
    @ObservationIgnored private var failedRecording: Recording?
    /// The current recording's recorder.
    @ObservationIgnored private var recorder: (any ScrawlRecorder)?
    /// The recorder is starting (or asking permission) and the overlay is already up.
    public private(set) var isStarting = false
    @ObservationIgnored private var pendingFinish: Bool?
    @ObservationIgnored private let makeRecorder: @MainActor (ScrawlCaptureMethod, Bool) -> any ScrawlRecorder
    /// Optional startup measurements, called on the stage's own queue. Keep the callback cheap.
    @ObservationIgnored public var startupObserver: (@Sendable (ScrawlStartupEvent) -> Void)?
    @ObservationIgnored var startupTrace: StartupTrace?
    @ObservationIgnored private var discardID = 0
    @ObservationIgnored private var discarded: FinishedRecording?
    /// Undo was tapped before the recorder finished writing this discard's movie.
    @ObservationIgnored private var undoneDiscardID: Int?
    /// The Scrawl address that answered when the current recording started.
    @ObservationIgnored private var reachableURL: URL?
    @ObservationIgnored private var lastSetupURLHandled: (url: URL, at: ContinuousClock.Instant)?
    /// Durable delivery to an HTTPS endpoint; exists only while paired with one.
    @ObservationIgnored private var outbox: Outbox?
    @ObservationIgnored private var outboxCredentials: (target: URL, token: String)?
    @ObservationIgnored private var outboxObservers: [any NSObjectProtocol] = []
    @ObservationIgnored private var pathMonitor: NWPathMonitor?
    @ObservationIgnored private var backgroundEventsCompletion: (() -> Void)?
    /// The HTTPS endpoint's /health answered 401: recordings are kept, marked failed.
    @ObservationIgnored private var pairingRevoked = false
    /// The outbox entry the strip is reporting on while it's sending or queued.
    @ObservationIgnored private var shownEntry: String?
    /// What the strip should say about the last recording sent to the HTTPS endpoint, until it
    /// lands or fails (or a new recording starts).
    @ObservationIgnored private var delivery: DeliveryTracker?
    /// From `NWPathMonitor`; assumed online until it says otherwise.
    @ObservationIgnored private var pathSatisfied = true
    /// Re-checks `delivery` when its upload would count as stalled.
    @ObservationIgnored private var stallCheck: Task<Void, Never>?
    /// An HTTPS upload landed while its "Sent to …" couldn't be shown (the app was in the
    /// background, or the strip was busy or gone): shown at the next chance.
    @ObservationIgnored private var pendingSent: IngestReply?
    /// The permanently failed outbox entry the strip's Share button shares.
    @ObservationIgnored private var failedEntry: String?
    @ObservationIgnored private var limitTask: Task<Void, Never>?
    @ObservationIgnored private var noticeID = 0
    @ObservationIgnored private var statusID = 0

    /// Whether to record the microphone with the screen.
    public var narration = true
    /// How recordings are captured: the app's own windows (the default; no screen-recording
    /// prompt) or ReplayKit (exact screen pixels, behind iOS's consent alert). Takes effect on the
    /// next recording.
    public var captureMethod: ScrawlCaptureMethod = .appFrames
    /// Whether to turn off iOS's three-finger text-editing gestures (undo, redo, cut, copy, paste,
    /// and their Undo · Cut · Copy · Redo bar) in this app while ScrawlKit's three-finger trigger
    /// is installed, so starting a recording with a text field focused doesn't open that bar.
    /// Only while ScrawlKit is active (paired, or `compiles = .onDevice`); set `false` to keep the
    /// system gestures.
    public var suppressesSystemEditingGestures = true {
      didSet { syncSystemIntegration() }
    }
    /// Windows with the three-finger trigger installed.
    @ObservationIgnored var threeFingerTriggers = 0 {
      didSet { syncSystemIntegration() }
    }
    /// Whether to show the share sheet when a recording stops (used as fallback if Mac ingest is unreachable).
    public var sharesWhenDone = true

    /// Where a recording becomes a `.scrawl` bundle.
    public enum Compiling: Sendable {
      /// Sent to Scrawl on the Mac (or its HTTPS endpoint), which builds it. The default.
      case onMac
      /// Built here on the phone with `ScrawlCore` (`SCRAWL_CORE=1`), the narration transcribed
      /// with Apple's on-device speech, then handed to `onBundle`. Transcribing needs
      /// `NSSpeechRecognitionUsageDescription` in the app's Info.plist; iOS asks the person once.
      case onDevice
    }

    /// Where recordings become bundles: on the Mac (the default) or on this phone. Takes effect on
    /// the next recording.
    public var compiles: Compiling = .onMac {
      didSet {
        guard compiles != oldValue else { return }
        notifyConfigurationChanged()
        #if SCRAWL_CORE
          if compiles == .onDevice { resumeInterruptedBuilds() }
        #endif
      }
    }
    /// Gets each bundle built on the phone (`compiles = .onDevice`): its `<id>.scrawl` folder in
    /// `directory`, with the movie and timeline inside. Without it, the share sheet offers the
    /// folder when `sharesWhenDone`.
    public var onBundle: (@MainActor (URL) -> Void)?
    /// Scrawl's experiments to try in bundles built on the phone, by name, as the Mac app's
    /// `experiments` in `config.json` (`"skipStillSwipes"`). Off unless named here.
    public var experiments: Set<String> = []
    /// The phone is building a recording's bundle (the strip says "Building…").
    private(set) var isBuildingOnPhone = false
    /// Interrupted on-device builds were looked for this launch (`resumeInterruptedBuilds`).
    @ObservationIgnored private var resumedBuilds = false
    /// Candidate Scrawl ingest URLs on the Mac (from Keychain or launch args / env).
    public var serverURLs: [URL] = [] {
      didSet { notifyConfigurationChanged() }
    }
    /// Bearer token for authenticating with Scrawl's ingest server.
    public var token: String? {
      didSet { notifyConfigurationChanged() }
    }
    /// Optional OpenCode session ID (`ses_...`) to route this app's captures to.
    public var sessionID: String?

    /// Whether this device has a valid Scrawl configuration (at least one `http`/`https` base URL and a token).
    public static var isConfigured: Bool {
      shared.isConfigured
    }

    public var isConfigured: Bool {
      !serverURLs.isEmpty && !(token?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }

    /// Whether ScrawlKit is in use here: paired with a Mac, or building bundles on the phone
    /// (`compiles = .onDevice`, which needs no Mac). Otherwise it stays inert.
    var isActive: Bool {
      #if SCRAWL_CORE
        isConfigured || compiles == .onDevice
      #else
        isConfigured
      #endif
    }

    static var isActive: Bool {
      shared.isActive
    }

    init(
      config: ScrawlConfig? = ScrawlConfig.resolved(),
      makeRecorder: @escaping @MainActor (ScrawlCaptureMethod, Bool) -> any ScrawlRecorder = { $0.makeRecorder(hosted: $1) }
    ) {
      self.makeRecorder = makeRecorder
      if let initial = config {
        serverURLs = initial.urls
        token = initial.token
        sessionID = initial.sessionID
      }
    }

    /// Handles a `<scheme>://scrawl?url=<urlencoded base>[&url=<second>]&token=<token>[&session=ses_…]`
    /// setup link or `<scheme>://scrawl?reset=1` reset link (also accepts the legacy `tether` host).
    /// Stores or clears the configuration in the Keychain, triggers haptic feedback, and returns
    /// `true` when `url` was a valid Scrawl link.
    @discardableResult
    public static func handleSetupURL(_ url: URL) -> Bool {
      guard let action = ScrawlConfig.parseSetupURL(url) else { return false }
      let now = ContinuousClock.now
      let isDuplicateEvent = shared.lastSetupURLHandled.map {
        $0.url == url && ($0.at.duration(to: now) < .milliseconds(250))
      } ?? false
      shared.lastSetupURLHandled = (url, now)

      switch action {
      case let .configure(newConfig):
        ScrawlConfig.saveToKeychain(newConfig)
        shared.applyConfig(ScrawlConfig.resolved(keychain: newConfig) ?? newConfig)
        if !isDuplicateEvent {
          UINotificationFeedbackGenerator().notificationOccurred(.success)
        }
      case .reset:
        ScrawlConfig.deleteFromKeychain()
        shared.applyConfig(ScrawlConfig.resolved(keychain: nil))
        if !isDuplicateEvent {
          UINotificationFeedbackGenerator().notificationOccurred(.warning)
        }
      }
      return true
    }

    @ObservationIgnored private var teamEndpoint: (url: URL, token: String)?
    /// Optional async supplement merged into `timeline.json` before a recording is written.
    @ObservationIgnored public var timelineSupplement: (@MainActor @Sendable ([String: Any]) async -> [String: Any])?
    /// Called on the main actor when a finished recording is handed to delivery. Returning `true`
    /// marks the recording as handled in-process without queuing an outbox upload.
    @ObservationIgnored public var onRecordingSubmitted: (@MainActor @Sendable (URL, URL) async -> Bool)?

    /// Configures or clears a team-feedback endpoint in memory without altering the device's
    /// saved personal Scrawl Keychain configuration. Passing `nil` restores the saved config.
    public func setTeamEndpoint(url: URL?, token: String?) {
      if let url, let token = token?.trimmingCharacters(in: .whitespacesAndNewlines), !token.isEmpty {
        teamEndpoint = (url, token)
        applyConfig(ScrawlConfig(urls: [url], token: token, sessionID: nil))
      } else {
        teamEndpoint = nil
        applyConfig(ScrawlConfig.resolved())
      }
    }

    private func applyConfig(_ config: ScrawlConfig?) {
      let effective = teamEndpoint.map { ScrawlConfig(urls: [$0.url], token: $0.token, sessionID: nil) } ?? config
      serverURLs = effective?.urls ?? []
      token = effective?.token
      sessionID = effective?.sessionID
      if !isActive {
        if isRecording || isStarting {
          isRecording = false
          isStarting = false
          isDrawing = false
          startedAt = nil
          pendingFinish = nil
          recorder?.cancel()
          recorder = nil
        }
        if isDiscardPending { finalizeDiscard() }
        hideOverlay()
      }
      notifyConfigurationChanged()
    }

    /// Posted on the next turn of the main queue: with @Observable, these properties' didSet runs
    /// inside init too, and a listener reading `Scrawl.shared` then would deadlock its creation.
    private func notifyConfigurationChanged() {
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        self.syncSystemIntegration()
        NotificationCenter.default.post(name: Self.configurationDidChangeNotification, object: self)
      }
    }

    /// The pieces that reach outside the app's own views (the editing-gesture override and the
    /// keyboard tracker) run only while ScrawlKit is active (`isActive`); otherwise nothing is
    /// installed or observed.
    func syncSystemIntegration() {
      SystemEditingGestures.setSuppressed(isActive && suppressesSystemEditingGestures && threeFingerTriggers > 0)
      KeyboardTracker.shared.setTracking(isActive)
      syncOutbox()
    }

    // MARK: - HTTPS Outbox

    /// The background `URLSession` identifier the outbox uploads on.
    public static var backgroundSessionIdentifier: String {
      "dev.kitlangton.scrawl.outbox.\(Bundle.main.bundleIdentifier ?? "app")"
    }

    /// Optional: forward `application(_:handleEventsForBackgroundURLSession:completionHandler:)`
    /// here so iOS can relaunch a killed app to finish reporting background uploads. Returns
    /// `false` (without calling `completionHandler`) for sessions that aren't ScrawlKit's. Without
    /// it, finished uploads are still picked up the next time the app launches.
    @discardableResult
    public static func handleBackgroundURLSessionEvents(identifier: String, completionHandler: @escaping () -> Void) -> Bool {
      guard identifier == backgroundSessionIdentifier else { return false }
      shared.syncOutbox()
      guard let outbox = shared.outbox else {
        completionHandler()
        return true
      }
      shared.backgroundEventsCompletion = completionHandler
      outbox.handleBackgroundEvents {
        Task { @MainActor in
          let completion = Scrawl.shared.backgroundEventsCompletion
          Scrawl.shared.backgroundEventsCompletion = nil
          completion?()
        }
      }
      return true
    }

    /// The configured HTTPS endpoint, if any. With one, recordings go through the outbox; plain
    /// `http` URLs are Macs on the LAN.
    private var hostedURL: URL? {
      serverURLs.first { $0.scheme?.lowercased() == "https" }
    }

    /// Creates the outbox (and its retry triggers) while paired with an HTTPS endpoint and tears it
    /// down otherwise: unpaired devices have no session, monitor, or files.
    private func syncOutbox() {
      let bearer = token?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
      guard isConfigured, let target = hostedURL, !bearer.isEmpty else {
        tearDownOutbox()
        return
      }
      let outbox = outbox ?? makeOutbox()
      outbox.setCredentials(target: target, token: bearer)
      if let previous = outboxCredentials, previous.target != target || previous.token != bearer {
        // A new link: what failed with the old one (revoked) gets another go.
        pairingRevoked = false
        outbox.retryFailed()
      }
      outboxCredentials = (target, bearer)
    }

    private func makeOutbox() -> Outbox {
      let outbox = Outbox(
        directory: URL.applicationSupportDirectory.appending(path: "ScrawlOutbox", directoryHint: .isDirectory),
        configuration: Outbox.backgroundConfiguration(identifier: Self.backgroundSessionIdentifier)
      ) { event in
        Task { @MainActor in Scrawl.shared.handle(event) }
      }
      self.outbox = outbox
      let center = NotificationCenter.default
      outboxObservers = [
        center.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { Scrawl.shared.outbox?.kick(force: true) }
        },
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
          MainActor.assumeIsolated { Scrawl.shared.showPendingSent() }
        },
      ]
      // Back online: try everything waiting now rather than at its backoff time.
      let monitor = NWPathMonitor()
      let reachability = Reachability()
      monitor.pathUpdateHandler = { [weak outbox] path in
        let satisfied = path.status == .satisfied
        if reachability.update(satisfied) { outbox?.kick(force: true) }
        Task { @MainActor in Scrawl.shared.pathChanged(satisfied: satisfied) }
      }
      monitor.start(queue: DispatchQueue(label: "dev.kitlangton.scrawl.outbox.path"))
      pathMonitor = monitor
      // Launch: whatever was waiting from earlier sessions.
      outbox.kick(force: true)
      return outbox
    }

    private func tearDownOutbox() {
      guard let outbox else { return }
      outbox.invalidate()
      self.outbox = nil
      outboxCredentials = nil
      pathMonitor?.cancel()
      pathMonitor = nil
      pathSatisfied = true
      delivery = nil
      stallCheck?.cancel()
      stallCheck = nil
      pendingSent = nil
      for observer in outboxObservers {
        NotificationCenter.default.removeObserver(observer)
      }
      outboxObservers = []
    }

    private func handle(_ event: Outbox.Event) {
      if case .failed(_, .unauthorized) = event { pairingRevoked = true }
      if var delivery, delivery.entry == event.id {
        delivery.apply(event, at: Date())
        self.delivery = delivery
        refreshDelivery()
        return
      }
      switch event {
      case let .sent(_, reply):
        // Sent from an earlier recording or launch: say so when there's a chance.
        pendingSent = reply
        showPendingSent()
      case let .failed(id, failure):
        guard isIdleForStatus else { return }
        Task { await presentFailure(ScrawlStrip.Failure(failure), entry: id) }
      case .started, .progress, .retrying:
        break
      }
    }

    private func pathChanged(satisfied: Bool) {
      guard satisfied != pathSatisfied else { return }
      pathSatisfied = satisfied
      delivery?.setPath(satisfied: satisfied, at: Date())
      refreshDelivery()
    }

    /// Nothing is recording or on the strip, and the app is in front.
    private var isIdleForStatus: Bool {
      UIApplication.shared.applicationState == .active && !isRecording && !isStarting
        && uploadStatus == .idle && !isDiscardPending
    }

    /// Whether the strip is showing the tracked upload's "Sending…" or "Queued".
    private var showsDelivery: Bool {
      delivery != nil && shownEntry == delivery?.entry && (uploadStatus == .sending || uploadStatus == .queued)
    }

    /// Puts what `delivery` says on the strip (while it's showing it) and finishes it once it has
    /// landed or failed.
    private func refreshDelivery() {
      stallCheck?.cancel()
      stallCheck = nil
      guard let delivery else { return }
      let now = Date()
      let display = delivery.display(at: now)
      let showing = showsDelivery
      switch display {
      case let .sending(progress), let .stillSending(progress):
        sendProgress = progress
        isSendStalled = display == .stillSending(progress: progress)
        if showing, uploadStatus == .queued {
          // A retry started on a working network: back to "Sending…".
          statusID += 1
          uploadStatus = .sending
        }
        if let next = delivery.nextChange(after: now) {
          stallCheck = Task { [weak self] in
            try? await Task.sleep(for: .seconds(next.timeIntervalSince(now) + 0.05))
            guard !Task.isCancelled else { return }
            self?.refreshDelivery()
          }
        }
      case .queued:
        if showing, uploadStatus == .sending { presentQueued() }
      case let .sent(reply):
        self.delivery = nil
        if showing, UIApplication.shared.applicationState == .active {
          shownEntry = nil
          Task { await presentSent(reply) }
        } else {
          // Shown in place (if the strip is still up) or on its own when the app is next in front.
          pendingSent = reply
          showPendingSent()
        }
      case let .failed(failure):
        self.delivery = nil
        guard showing || isIdleForStatus else { return }
        shownEntry = nil
        Task { await presentFailure(ScrawlStrip.Failure(failure), entry: delivery.entry) }
      }
    }

    /// Shows a "Sent to …" that couldn't be shown when the upload landed: in place of a strip still
    /// saying "Sending…" for it, or by bringing the strip back briefly.
    private func showPendingSent() {
      guard let reply = pendingSent, UIApplication.shared.applicationState == .active,
            !isRecording, !isStarting, !isDiscardPending
      else { return }
      let staleStatus = delivery == nil && shownEntry != nil && (uploadStatus == .sending || uploadStatus == .queued)
      guard uploadStatus == .idle || staleStatus else { return }
      shownEntry = nil
      showOverlay()
      Task { await presentSent(reply) }
    }

    /// Logs something the app did ("Done tapped", "rest started"), placed on the recording's timeline.
    public static func event(_ label: String) {
      MainActor.assumeIsolated { shared.record(event: label) }
    }

    /// Starts recording, or sends the current recording.
    public func toggle() {
      guard isActive else { return }
      isRecording || isStarting ? stop() : start()
    }

    public func toggleDrawing() {
      guard isRecording || isStarting else { return }
      isDrawing.toggle()
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    public func start() {
      guard isActive, !isRecording, !isStarting else { return }
      start(trace: startupObserver.map { StartupTrace(receive: $0) })
    }

    /// Starts the measurement at recognition, before the trigger's feedback work.
    func triggerRecognized() {
      guard isActive else { return }
      let trace = !isRecording && !isStarting ? startupObserver.map { StartupTrace(receive: $0) } : nil
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      if isRecording || isStarting { stop() } else { start(trace: trace) }
    }

    private func start(trace: StartupTrace?) {
      guard isActive, !isRecording, !isStarting else { return }
      prewarmExpiry?.cancel()
      prewarmExpiry = nil
      startupTrace = trace
      let hosted = hostedURL != nil
      if isDiscardPending { finalizeDiscard() }
      markers = []
      events = []
      touchLog = []
      strokeLog = []
      completedStrokes = []
      overlay?.clearInk(animated: false)
      lastTouchLog = [:]
      touchLabels = [:]
      // Fingers already down (the three-finger trigger) are never drawn or logged: only touches
      // that begin after this point get an ID.
      touchIDs = [:]
      swipeStarts = [:]
      nextTouchID = 1
      holdStarts = [:]
      draggingTouchIDs = []
      trackedScrollViews = []
      lastTouchMoveMediaTime = nil
      isDrawing = false
      uploadStatus = .idle
      failedRecording = nil
      failedEntry = nil
      // An upload still going is no longer on the strip; when it lands it says so on its own.
      shownEntry = nil
      delivery = nil
      stallCheck?.cancel()
      stallCheck = nil
      statusID += 1
      isStarting = true
      pendingFinish = nil
      startedAt = Date()
      showOverlay()
      preparedAt = nil
      // Rendering has no dependency on permission, engine construction, disk, or the encoder.
      let recorder = makeRecorder(captureMethod, hosted)
      recorder.startupTrace = startupTrace
      startupTrace?.mark("recorderCreated")
      guard recorder.isAvailable else {
        isStarting = false
        startedAt = nil
        hideOverlay()
        return
      }
      let trace = startupTrace
      self.recorder = recorder
      recorder.onInterruption = { [weak self] in self?.stop() }
      let begin: @MainActor () -> Void = { [weak self, weak recorder] in
        guard let self, let recorder, self.recorder === recorder, self.isStarting else { return }
        Task {
          if hosted {
            trace?.mark("outboxBegin")
            self.syncOutbox()
            trace?.mark("outboxEnd")
            if self.pairingRevoked { self.showNotice(.linkExpired, for: .seconds(5)) }
          }
        }
        // Raise local-network consent at start, but only after the prepared UI is presented.
        Task { await self.findMac() }
        recorder.start(microphone: self.narration) { [weak self, weak recorder] result in
          guard let self, let recorder, self.recorder === recorder, self.isStarting else { return }
          self.isStarting = false
          switch result {
          case let .success(startedAt):
            self.startupTrace?.mark("ready")
            // Touches and drawing are accepted while the hardware starts. Keep their timestamps
            // on the movie's clock (the app-frame recorder begins its clock at start invocation).
            let shift = self.startedAt?.timeIntervalSince(startedAt) ?? 0
            self.rebaseEarlyInput(by: shift)
            self.startedAt = startedAt
            KeyboardTracker.shared.recordingWillBegin()
            ScrawlAppContext.shared.begin(at: startedAt)
            self.isRecording = true
            if hosted { self.startLimit(for: recorder.method, startedAt: startedAt) }
            if let discarding = self.pendingFinish {
              self.pendingFinish = nil
              self.finish(discarding: discarding)
            }
          case let .failure(error):
            print("Scrawl: could not start recording: \(error.localizedDescription)")
            self.recorder = nil
            self.startedAt = nil
            self.pendingFinish = nil
            self.isDrawing = false
            self.hideOverlay()
          }
        }
      }
      if let overlay { overlay.afterFirstPresentation(begin) } else { begin() }
    }

    /// Stops recording and sends it to your Mac (the strip's Send).
    public func stop() {
      finish(discarding: false)
    }

    /// Stops recording without sending it. The strip offers Undo (which sends it) for a few
    /// seconds; after that the recording's files are deleted.
    public func discard() {
      guard isRecording || isStarting else { return }
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
      finish(discarding: true)
    }

    /// The discarded toast's Undo: sends the recording after all.
    func undoDiscard() {
      guard isDiscardPending else { return }
      isDiscardPending = false
      uploadStatus = .sending
      UIImpactFeedbackGenerator(style: .light).impactOccurred()
      if let finished = discarded {
        discarded = nil
        Task { await self.deliver(finished) }
      } else {
        undoneDiscardID = discardID
      }
    }

    private func finish(discarding: Bool) {
      if isStarting {
        pendingFinish = discarding
        return
      }
      guard isRecording, let startedAt, let recorder else { return }
      self.recorder = nil
      isRecording = false
      isDrawing = false
      limitTask?.cancel()
      limitTask = nil
      notice = nil
      let discardToken = discarding ? beginUndoWindow() : nil
      if discardToken == nil { uploadStatus = .sending }
      // Recordings for an HTTPS endpoint (often showing private data) stay out of Documents,
      // which is backed up: they're staged in tmp and live on only inside the outbox.
      let directory = hostedURL != nil ? Self.stagingDirectory : Self.directory
      try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let stamp = startedAt.formatted(.iso8601.year().month().day().timeSeparator(.omitted).time(includingFractionalSeconds: false))
        .replacingOccurrences(of: ":", with: "")
      let movie = directory.appending(path: "\(stamp).mp4")
      let timeline = directory.appending(path: "\(stamp).json")
      let readyMarker = directory.appending(path: "\(stamp).ready")
      let sentMarker = directory.appending(path: "\(stamp).sent")
      let appName = (Bundle.main.infoDictionary?["CFBundleDisplayName"] as? String)
        ?? (Bundle.main.infoDictionary?["CFBundleName"] as? String)
        ?? (Bundle.main.bundleIdentifier ?? "App")
      var info: [String: Any] = [
        "app": appName,
        "bundleId": Bundle.main.bundleIdentifier ?? "",
        "version": Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "",
        "device": UIDevice.current.name,
        "system": "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)",
        // Touches are in this window's points (portrait or landscape as it was recorded).
        "screen": [
          "width": touchWindow?.bounds.width ?? 0, "height": touchWindow?.bounds.height ?? 0,
          "scale": touchWindow?.traitCollection.displayScale ?? 0,
        ],
        "recordedAt": startedAt.ISO8601Format(),
        "duration": Self.round(Date().timeIntervalSince(startedAt)),
        "markers": markers,
        "events": events,
        "touches": touchLog,
        "strokes": strokeLog,
        "capture": recorder.method.rawValue,
      ]
      if let sessionID, !sessionID.isEmpty {
        info["session"] = sessionID
      }
      info.merge(ScrawlAppContext.shared.end()) { current, _ in current }
      self.startedAt = nil
      completedStrokes = []
      overlay?.clearInk(animated: true)
      recorder.stop(writingTo: movie) { [weak self] error in
        Task { @MainActor in
          guard let self else { return }
          if let error {
            print("Scrawl: recording failed: \(error.localizedDescription)")
            self.uploadStatus = .idle
            if discardToken == self.discardID { self.isDiscardPending = false }
            await self.hideOverlayAfterExit()
            return
          }
          var info = info
          info["audio"] = recorder.recordsAudio
          if let supplement = self.timelineSupplement {
            for (key, value) in await supplement(info) {
              info[key] = value
            }
          }
          let timelineData = (try? JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys])) ?? Data()
          try? timelineData.write(to: timeline)
          let finished = FinishedRecording(
            recording: Recording(movie: movie, timeline: timeline),
            timelineData: timelineData,
            readyMarker: readyMarker,
            sentMarker: sentMarker,
            recordedAt: startedAt.ISO8601Format()
          )
          guard let discardToken else {
            await self.deliver(finished)
            return
          }
          if self.undoneDiscardID == discardToken {
            self.undoneDiscardID = nil
            await self.deliver(finished)
          } else if self.isDiscardPending, self.discardID == discardToken {
            self.discarded = finished
          } else {
            Self.delete(finished)
          }
        }
      }
    }

    /// Shows the discarded toast and deletes the recording when its Undo window runs out.
    private func beginUndoWindow() -> Int {
      discardID += 1
      let id = discardID
      discarded = nil
      undoneDiscardID = nil
      isDiscardPending = true
      Task { [weak self] in
        try? await Task.sleep(for: Self.undoWindow)
        guard let self, self.isDiscardPending, self.discardID == id else { return }
        self.finalizeDiscard()
        await self.hideOverlayAfterExit()
      }
      return id
    }

    /// Ends the Undo window now. A recording still being written is deleted when it lands.
    private func finalizeDiscard() {
      isDiscardPending = false
      if let discarded { Self.delete(discarded) }
      discarded = nil
    }

    private static func delete(_ finished: FinishedRecording) {
      for file in [
        finished.recording.movie, finished.recording.timeline, finished.readyMarker, finished.sentMarker,
        deliveryMarker(for: finished.sentMarker),
      ] {
        try? FileManager.default.removeItem(at: file)
      }
    }

    private func deliver(_ finished: FinishedRecording) async {
      if let onRecordingSubmitted, await onRecordingSubmitted(finished.recording.timeline, finished.recording.movie) {
        uploadStatus = .idle
        Self.delete(finished)
        await hideOverlayAfterExit()
        return
      }
      if compiles == .onDevice {
        #if SCRAWL_CORE
          await deliverOnDevice(finished)
          return
        #else
          print("Scrawl: compiles = .onDevice needs ScrawlCore (build with SCRAWL_CORE=1); sending to the Mac")
        #endif
      }
      if hostedURL != nil {
        await deliverHosted(finished)
        return
      }
      // `.ready` lets the Mac pick the recording up over USB if the network fails.
      try? Data("ready".utf8).write(to: finished.readyMarker)
      recordings.append(finished.recording)
      await deliverToMacOrShare(finished)
    }

    /// Marks this moment ("look here").
    public func mark() {
      guard let startedAt else { return }
      markers.append(Self.round(Date().timeIntervalSince(startedAt)))
      UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    }

    /// Where recordings for a Mac are kept: Documents/Scrawl. (Recordings for an HTTPS endpoint
    /// are kept in the outbox, Application Support/ScrawlOutbox, until they're sent.)
    public static var directory: URL {
      URL.documentsDirectory.appending(path: "Scrawl", directoryHint: .isDirectory)
    }

    /// Where a recording for an HTTPS endpoint is written before it becomes an outbox entry.
    private static var stagingDirectory: URL {
      URL.temporaryDirectory.appending(path: "ScrawlStaging", directoryHint: .isDirectory)
    }

    /// Warns shortly before an HTTPS recording's length cap, then sends it.
    private func startLimit(for method: ScrawlCaptureMethod, startedAt: Date) {
      limitTask?.cancel()
      let limit = Self.hostedRecordingLimit(for: method)
      limitTask = Task { [weak self] in
        try? await Task.sleep(for: limit - Self.limitWarning)
        guard let self, !Task.isCancelled, self.isRecording, self.startedAt == startedAt else { return }
        self.showNotice(.init("Length limit", detail: "Sending in \(Self.limitWarning.components.seconds) s"), for: nil)
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        try? await Task.sleep(for: Self.limitWarning)
        guard !Task.isCancelled, self.isRecording, self.startedAt == startedAt else { return }
        self.stop()
      }
    }

    /// Shows `text` beside the strip while recording, for `duration` (or until it stops).
    private func showNotice(_ text: ScrawlStrip.Notice, for duration: Duration?) {
      noticeID += 1
      let id = noticeID
      notice = text
      guard let duration else { return }
      Task { [weak self] in
        try? await Task.sleep(for: duration)
        guard let self, self.noticeID == id else { return }
        self.notice = nil
      }
    }

    // MARK: - Freehand Drawing on the Overlay

    /// A finished stroke from the overlay: its smoothed samples (never predicted ones) go on the
    /// timeline at their touches' own times. Returns its index there.
    func record(stroke samples: [InkSample]) -> Int? {
      guard let startedAt, let first = samples.first, let last = samples.last else { return nil }
      let offset = Self.recordingOffset(from: startedAt)
      let points = samples.map { [Self.round($0.point.x), Self.round($0.point.y), Self.round(max($0.time + offset, 0))] }
      completedStrokes.append(samples.map(\.point))
      strokeLog.append([
        "t0": Self.round(max(first.time + offset, 0)),
        "t1": Self.round(max(last.time + offset, 0)),
        "points": points,
      ])
      return strokeLog.count - 1
    }

    /// The overlay started fading these strokes at `uptime`: `fadeAt` on the timeline says when
    /// they left the screen.
    func recordFade(of strokes: [Int], at uptime: TimeInterval) {
      guard let startedAt else { return }
      let t = Self.round(max(uptime + Self.recordingOffset(from: startedAt), 0))
      for index in strokes where strokeLog.indices.contains(index) {
        strokeLog[index]["fadeAt"] = t
      }
    }

    /// Add to a touch timestamp (seconds since boot) to get seconds into the recording.
    private static func recordingOffset(from startedAt: Date) -> TimeInterval {
      Date().timeIntervalSince(startedAt) - ProcessInfo.processInfo.systemUptime
    }

    // MARK: - Internals

    /// Cheap preparation when a configured host sees a potential trigger. No audio, capture,
    /// permission request or disk work; the hidden view is released if the press is abandoned.
    public func prepareToStart() {
      guard isActive, !isRecording, !isStarting, overlay == nil,
            let scene = Self.activeScene else { return }
      let start = CACurrentMediaTime()
      preparedAt = Date()
      overlay = OverlayWindow(windowScene: scene, scrawl: self)
      overlay?.prepareLayout()
      startupObserver?(.init(stage: "prewarm", milliseconds: (CACurrentMediaTime() - start) * 1000, onMainThread: true))
      prewarmExpiry = Task { [weak self] in
        try? await Task.sleep(for: .seconds(2))
        guard !Task.isCancelled, let self, !self.isStarting, !self.isRecording else { return }
        self.hideOverlay()
      }
    }

    private func rebaseEarlyInput(by shift: TimeInterval) {
      guard shift != 0 else { return }
      markers = markers.map { Self.round(max(0, $0 + shift)) }
      for index in events.indices {
        if let t = events[index]["t"] as? Double { events[index]["t"] = Self.round(max(0, t + shift)) }
      }
      for index in touchLog.indices {
        if let t = touchLog[index]["t"] as? Double { touchLog[index]["t"] = Self.round(max(0, t + shift)) }
      }
      for index in strokeLog.indices {
        for key in ["t0", "t1", "fadeAt"] {
          if let t = strokeLog[index][key] as? Double { strokeLog[index][key] = Self.round(max(0, t + shift)) }
        }
        if let points = strokeLog[index]["points"] as? [[Double]] {
          strokeLog[index]["points"] = points.map { [$0[0], $0[1], Self.round(max(0, $0[2] + shift))] }
        }
      }
    }

    func record(event label: String) {
      guard let startedAt else { return }
      events.append(["t": Self.round(Date().timeIntervalSince(startedAt)), "label": label])
    }

    /// Every finger has lifted.
    func touchesReset() {
      touchIDs = [:]
      touchLabels = [:]
      holdStarts = [:]
      draggingTouchIDs = []
    }

    /// Whether the user is actively scrolling, dragging, decelerating a scroll view, or drawing ink.
    var isInteractingForCapture: Bool {
      if overlay?.isActivelyDrawing == true || !draggingTouchIDs.isEmpty {
        return true
      }
      if RunLoop.main.currentMode == .tracking {
        return true
      }
      for entry in trackedScrollViews {
        if let scroll = entry.value, !scroll.isHidden, scroll.window != nil,
           scroll.isTracking || scroll.isDragging || scroll.isDecelerating
        {
          return true
        }
      }
      return false
    }

    /// Touches go to the timeline while recording (moves at most 30 a second). A touch that began
    /// before the recording was started (the trigger's fingers) is ignored.
    func record(touch: UITouch, in window: UIWindow, phase: UITouch.Phase) {
      guard isRecording || isStarting else { return }
      touchWindow = window
      let key = ObjectIdentifier(touch)
      let id: Int
      if let known = touchIDs[key] {
        id = known
      } else if phase == .began {
        id = nextTouchID
        nextTouchID += 1
        touchIDs[key] = id
      } else {
        return
      }
      if phase == .moved {
        lastTouchMoveMediaTime = CACurrentMediaTime()
        draggingTouchIDs.insert(id)
        swipeStarts[id]?.reach(touch.location(in: window))
        if holdStarts[id] == nil {
          guard let startedAt else { return }
          let t = Date().timeIntervalSince(startedAt)
          if let last = lastTouchLog[id], t - last < 1.0 / 30 { return }
          lastTouchLog[id] = t
          let point = touch.location(in: window)
          var entry: [String: Any] = [
            "t": Self.round(t),
            "id": id,
            "x": Self.round(point.x),
            "y": Self.round(point.y),
            "phase": "move",
          ]
          if let label = touchLabels[id], !label.isEmpty {
            entry["label"] = label
          }
          touchLog.append(entry)
          return
        }
      }
      let point = touch.location(in: window)
      if phase == .began {
        let leaf = touch.view ?? window.hitTest(point, with: nil)
        trackScrollViews(from: leaf)
        swipeStarts[id] = Self.scrollView(above: leaf).map {
          SwipeStart(scroll: WeakScrollView($0), offset: $0.contentOffset, point: point)
        }
        if let label = Self.hitLabel(at: point, in: window) {
          touchLabels[id] = label
        } else {
          touchLabels.removeValue(forKey: id)
        }
      }
      let label = touchLabels[id]
      let ended = phase == .ended || phase == .cancelled
      var swipeStart: SwipeStart?
      if ended {
        touchIDs[key] = nil
        touchLabels.removeValue(forKey: id)
        if draggingTouchIDs.remove(id) != nil { swipeStart = swipeStarts[id] }
        swipeStarts[id] = nil
      }
      guard let startedAt else { return }
      trackHold(id: id, at: point, phase: phase)
      let t = Date().timeIntervalSince(startedAt)
      if phase == .moved, let last = lastTouchLog[id], t - last < 1.0 / 30 { return }
      lastTouchLog[id] = t
      let name = switch phase {
      case .began: "down"
      case .moved: "move"
      case .ended: "up"
      case .cancelled: "cancel"
      default: "other"
      }
      var entry: [String: Any] = [
        "t": Self.round(t),
        "id": id,
        "x": Self.round(point.x),
        "y": Self.round(point.y),
        "phase": name,
      ]
      if let label, !label.isEmpty {
        entry["label"] = label
      }
      touchLog.append(entry)
      if let swipeStart {
        settleSwipe(id: id, entry: touchLog.count - 1, from: swipeStart, recording: startedAt)
      }
    }

    /// The nearest scroll view at or above `view` that scrolls.
    private static func scrollView(above view: UIView?) -> UIScrollView? {
      var current = view
      while let candidate = current {
        if let scroll = candidate as? UIScrollView, scroll.isScrollEnabled { return scroll }
        current = candidate.superview
      }
      return nil
    }

    /// Once the swipe's scroll view has come to rest (up to 3 s), marks the swipe's last sample
    /// `still` when its content is back where it started: a list pulled and let go, or swiped past
    /// its end, which changed nothing on screen. A swipe across the way it scrolls (a row's swipe
    /// actions, a page) isn't marked, since its scroll view can't say.
    private func settleSwipe(id: Int, entry index: Int, from start: SwipeStart, recording: Date) {
      guard let scroll = start.scroll.value else { return }
      // Under the tap slop (`touch_events`) it's a tap or a long press, not a swipe.
      guard start.travel >= 12 else { return }
      let (dx, dy) = (start.farthest.dx, start.farthest.dy)
      let size = scroll.contentSize
      let inset = scroll.adjustedContentInset
      let scrollsDown = scroll.alwaysBounceVertical || size.height + inset.top + inset.bottom > scroll.bounds.height
      let scrollsAcross = scroll.alwaysBounceHorizontal || size.width + inset.left + inset.right > scroll.bounds.width
      guard dy >= dx ? scrollsDown : scrollsAcross else { return }
      Task { [weak self, weak scroll] in
        var last: CGPoint?
        for _ in 0 ..< 30 {
          try? await Task.sleep(for: .milliseconds(100))
          guard let self, let scroll, self.startedAt == recording else { return }
          let moving = scroll.isTracking || scroll.isDragging || scroll.isDecelerating
          let offset = scroll.contentOffset
          if !moving, offset == last {
            let still = hypot(offset.x - start.offset.x, offset.y - start.offset.y) < 1
            guard index < self.touchLog.count, self.touchLog[index]["id"] as? Int == id else { return }
            self.touchLog[index]["still"] = still
            return
          }
          last = offset
        }
      }
    }

    private func trackScrollViews(from leaf: UIView?) {
      trackedScrollViews.removeAll { $0.value == nil || $0.value?.window == nil }
      var current = leaf
      while let view = current {
        if let scroll = view as? UIScrollView, !trackedScrollViews.contains(where: { $0.value === scroll }) {
          trackedScrollViews.append(WeakScrollView(scroll))
        }
        current = view.superview
      }
    }

    /// A light tick when a finger has held still for a moment, so a deliberate "this one" press
    /// is felt.
    private func trackHold(id: Int, at point: CGPoint, phase: UITouch.Phase) {
      switch phase {
      case .began:
        holdStarts[id] = point
        Task { [weak self] in
          try? await Task.sleep(for: .milliseconds(420))
          guard let self, self.isRecording, self.holdStarts.removeValue(forKey: id) != nil else { return }
          UIImpactFeedbackGenerator(style: .rigid).impactOccurred()
        }
      case .moved:
        if let start = holdStarts[id], hypot(point.x - start.x, point.y - start.y) > 12 {
          holdStarts[id] = nil
        }
      default:
        holdStarts[id] = nil
      }
    }

    /// Finds a concise human-readable label for the UIView under `point` in `window`.
    private static func hitLabel(at point: CGPoint, in window: UIWindow) -> String? {
      guard let hit = window.hitTest(point, with: nil) else { return nil }
      var current: UIView? = hit
      for _ in 0 ..< 7 {
        guard let view = current else { break }
        if let ax = cleanedLabel(view.accessibilityLabel) { return ax }
        if let button = view as? UIButton {
          if let title = cleanedLabel(button.currentTitle ?? button.configuration?.title) {
            return title
          }
        }
        if let label = view as? UILabel, let text = cleanedLabel(label.text) {
          return text
        }
        // Check direct or shallow labels inside compact controls/rows (<= 110pt tall).
        if view.bounds.height <= 110 {
          if let childText = nearestChildLabel(in: view, at: view.convert(point, from: window), depth: 3) {
            return childText
          }
        }
        current = view.superview
      }
      return accessibilityLabel(at: point, in: window, above: hit)
    }

    /// SwiftUI draws its text without UILabels, so the walk above finds nothing in a SwiftUI view;
    /// the accessibility elements it makes for VoiceOver carry the labels instead. The smallest
    /// labeled element under `point`, from the nearest of `hit`'s ancestors that has one.
    private static func accessibilityLabel(at point: CGPoint, in window: UIWindow, above hit: UIView) -> String? {
      // Accessibility frames are in screen coordinates.
      let onScreen = window.convert(point, to: window.screen.coordinateSpace)
      var visited = 0
      // Fetched one at a time, so a container with thousands of elements costs only the budget.
      func children(of element: NSObject) -> (count: Int, at: (Int) -> NSObject?) {
        if let elements = element.accessibilityElements {
          return (elements.count, { elements[$0] as? NSObject })
        }
        let count = element.accessibilityElementCount()
        guard count > 0, count != NSNotFound else { return (0, { _ in nil }) }
        return (count, { element.accessibilityElement(at: $0) as? NSObject })
      }
      func smallest(in element: NSObject, depth: Int) -> (area: CGFloat, label: String)? {
        guard depth > 0 else { return nil }
        var best: (area: CGFloat, label: String)?
        let elements = children(of: element)
        for index in 0 ..< elements.count {
          guard visited < 400 else { break }
          visited += 1
          guard let child = elements.at(index) else { continue }
          let frame = child.accessibilityFrame
          // A container with no frame of its own can still hold the element under the point.
          if !frame.isEmpty, !frame.contains(onScreen) { continue }
          if !frame.isEmpty, let label = cleanedLabel(child.accessibilityLabel),
             best == nil || frame.width * frame.height < best!.area
          {
            best = (frame.width * frame.height, label)
          }
          if let inner = smallest(in: child, depth: depth - 1), best == nil || inner.area < best!.area {
            best = inner
          }
        }
        return best
      }
      var current: UIView? = hit
      for _ in 0 ..< 7 {
        guard let view = current else { break }
        if let found = smallest(in: view, depth: 6) { return found.label }
        current = view.superview
      }
      return nil
    }

    private static func nearestChildLabel(in root: UIView, at point: CGPoint, depth: Int) -> String? {
      guard depth >= 0, !root.isHidden, root.alpha > 0.05 else { return nil }
      var best: (CGFloat, String)?
      func walk(_ view: UIView, _ remaining: Int) {
        guard remaining >= 0, !view.isHidden, view.alpha > 0.05 else { return }
        if let label = view as? UILabel, let text = cleanedLabel(label.text) {
          let rect = view.convert(view.bounds, to: root)
          let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
          let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
          let dist = hypot(dx, dy)
          if best == nil || dist < best!.0 {
            best = (dist, text)
          }
        }
        for sub in view.subviews {
          walk(sub, remaining - 1)
        }
      }
      walk(root, depth)
      return best?.1
    }

    private static func cleanedLabel(_ raw: String?) -> String? {
      guard let raw else { return nil }
      let compact = raw.split(whereSeparator: \.isWhitespace).joined(separator: " ")
      guard !compact.isEmpty else { return nil }
      return compact.count > 48 ? String(compact.prefix(48)) + "…" : compact
    }

    // MARK: - Mac / Agent Delivery

    private func deliverToMacOrShare(_ finished: FinishedRecording) async {
      let recording = finished.recording
      let timelineData = finished.timelineData
      let readyMarker = finished.readyMarker
      let sentMarker = finished.sentMarker
      let movieData = (try? Data(contentsOf: recording.movie)) ?? Data()
      var framed = Data()
      var len = UInt32(timelineData.count).bigEndian
      withUnsafeBytes(of: &len) { framed.append(contentsOf: $0) }
      framed.append(timelineData)
      framed.append(movieData)

      // The address that answered when recording started goes alone, and gets longer to answer;
      // otherwise try them all.
      let urls = reachableURL.map { [$0] } ?? serverURLs
      let replyWait = reachableURL == nil ? 15 : 45
      let bearerToken = token?.trimmingCharacters(in: .whitespacesAndNewlines)
      let deliveryMarker = Self.deliveryMarker(for: sentMarker)
      let deliveredReply: IngestReply? = await withTaskGroup(of: IngestReply?.self) { group in
        // 1. Direct HTTP POST to Scrawl on the Mac (LAN + Tailscale in parallel). The Mac
        // answers once it has built the bundle and tried to send it on (or after `replyWait`).
        if let bearerToken, !bearerToken.isEmpty {
          for base in urls {
            group.addTask {
              await Self.postIngest(base: base, payload: framed, token: bearerToken, replyWait: replyWait)
            }
          }
        }
        // 2. USB / CoreDevice fallback: Scrawl.app copies the files with `devicectl`, builds the
        // bundle, and writes `<stamp>.delivery` (its reply) and then `<stamp>.sent` back.
        group.addTask {
          for _ in 0 ..< 75 {
            try? await Task.sleep(for: .milliseconds(400))
            if let data = try? Data(contentsOf: sentMarker),
               let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
               !text.isEmpty
            {
              return IngestReply(sentMarker: text, delivery: try? Data(contentsOf: deliveryMarker))
            }
          }
          return nil
        }

        for await result in group {
          if let target = result {
            group.cancelAll()
            return target
          }
        }
        return nil
      }

      if let reply = deliveredReply {
        try? FileManager.default.removeItem(at: readyMarker)
        try? FileManager.default.removeItem(at: sentMarker)
        try? FileManager.default.removeItem(at: deliveryMarker)
        await presentSent(reply)
      } else {
        // Keep `.ready` so the Mac still picks the recording up the next time the phone is cabled.
        // The share sheet only opens if the strip's Share button is tapped.
        await presentFailure(.unreachable, recording: recording)
      }
    }

    #if SCRAWL_CORE
      /// On the phone: transcribe, build the bundle into `directory`, move the movie and timeline
      /// into it, and hand it to `onBundle` (or the share sheet). A failed build keeps the
      /// recording, with Share.
      /// `recovered`: a build iOS ended last time (`resumeInterruptedBuilds`), which goes to
      /// `onBundle` only, with no strip or share sheet.
      private func deliverOnDevice(_ finished: FinishedRecording, recovered: Bool = false) async {
        let recording = finished.recording
        // Until this build is handled, so one iOS ends (the app killed in the background) is
        // finished at the next launch.
        let marker = Self.buildMarker(for: recording.movie)
        try? Data().write(to: marker)
        defer { try? FileManager.default.removeItem(at: marker) }
        let timeline = finished.timelineData
        let info = (try? JSONSerialization.jsonObject(with: timeline)) as? [String: Any]
        let narrated = info?["audio"] as? Bool ?? false
        let id = recording.movie.deletingPathExtension().lastPathComponent
        let createdAt = (try? Date(finished.recordedAt, strategy: .iso8601)) ?? Date()
        let directory = Self.directory
        let experiments = self.experiments
        if !recovered { isBuildingOnPhone = true }
        // Leaving the app ends an app-frames recording and starts this; ask for time to finish.
        // A build longer than iOS allows fails and keeps the recording, with Share.
        let background = BackgroundTime(named: "Scrawl bundle")
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
        background.end()
        if !recovered { isBuildingOnPhone = false }
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
          // Paired with an HTTPS receiver: the bundle goes there too, and the strip follows it.
          let uploads = hostedURL != nil && isConfigured
          if let onBundle {
            onBundle(bundle)
          } else if sharesWhenDone, !recovered, !uploads {
            Self.share([bundle])
          }
          if uploads {
            await sendBundle(bundle, recordedAt: finished.recordedAt, shown: !recovered)
          } else if !recovered {
            await presentSent(.onPhone)
          }
        case let .failure(error):
          print("Scrawl: building the bundle on the phone failed: \(error)")
          recordings.append(recording)
          if !recovered { await presentFailure(.notBuilt, recording: recording) }
        }
      }

      /// Queues a bundle built on the phone for the HTTPS receiver (`scrawl-server`'s
      /// `POST /bundles`) and, when `shown`, follows its upload on the strip as `deliverHosted`
      /// does. It's on the phone already, so a bundle that can't be queued is still "Saved on
      /// this iPhone".
      private func sendBundle(_ bundle: URL, recordedAt: String, shown: Bool) async {
        syncOutbox()
        guard let outbox, let target = hostedURL else {
          if shown { await presentSent(.onPhone) }
          return
        }
        let entry: Outbox.Entry
        do {
          entry = try await outbox.enqueue(
            bundle: bundle, target: target, recordedAt: recordedAt,
            failure: pairingRevoked ? .unauthorized : nil
          )
        } catch {
          print("Scrawl: could not save the bundle for sending: \(error)")
          if shown { await presentSent(.onPhone) }
          return
        }
        guard shown else {
          outbox.kick(force: true)
          return
        }
        await follow(entry, on: outbox)
      }

      private nonisolated static func buildMarker(for movie: URL) -> URL {
        movie.deletingPathExtension().appendingPathExtension("building")
      }

      /// Builds again, once a launch, the recordings whose build iOS ended (a `.building` marker
      /// left beside the movie): the app was killed in the background before it finished.
      private func resumeInterruptedBuilds() {
        guard !resumedBuilds else { return }
        resumedBuilds = true
        let manager = FileManager.default
        var interrupted: [FinishedRecording] = []
        for directory in [Self.directory, Self.stagingDirectory] {
          let files = (try? manager.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
          for marker in files where marker.pathExtension == "building" {
            let stem = marker.deletingPathExtension()
            let timeline = stem.appendingPathExtension("json")
            guard let movie = ["mp4", "mov"].map({ stem.appendingPathExtension($0) })
              .first(where: { manager.fileExists(atPath: $0.path) }),
              let data = try? Data(contentsOf: timeline)
            else {
              try? manager.removeItem(at: marker)
              continue
            }
            let info = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            interrupted.append(FinishedRecording(
              recording: Recording(movie: movie, timeline: timeline),
              timelineData: data,
              readyMarker: stem.appendingPathExtension("ready"),
              sentMarker: stem.appendingPathExtension("sent"),
              recordedAt: info?["recordedAt"] as? String ?? ""
            ))
          }
        }
        guard !interrupted.isEmpty else { return }
        Task {
          for finished in interrupted {
            await self.deliverOnDevice(finished, recovered: true)
          }
        }
      }
    #endif

    /// Time iOS allows the app to keep running in the background, held until `end()` (or until
    /// iOS takes it back).
    @MainActor private final class BackgroundTime {
      private var id = UIBackgroundTaskIdentifier.invalid

      init(named name: String) {
        id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
          MainActor.assumeIsolated { self?.end() }
        }
      }

      func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
      }
    }

    /// HTTPS endpoint: the recording becomes an outbox entry (on disk before anything is sent) and
    /// the strip follows its upload: "Sending…" while it runs on a working network, however long
    /// that takes, "Queued" while offline or waiting to retry, then "Sent to …". The outbox keeps
    /// trying in the background until a 2xx.
    private func deliverHosted(_ finished: FinishedRecording) async {
      syncOutbox()
      guard let outbox, let target = hostedURL else {
        await presentFailure(.notSaved, recording: finished.recording)
        return
      }
      let entry: Outbox.Entry
      do {
        entry = try await outbox.enqueue(
          timeline: finished.timelineData,
          movie: finished.recording.movie,
          target: target,
          recordedAt: finished.recordedAt,
          failure: pairingRevoked ? .unauthorized : nil
        )
      } catch {
        // The staged files stay so Share still has them.
        print("Scrawl: could not save the recording for sending: \(error.localizedDescription)")
        await presentFailure(.notSaved, recording: finished.recording)
        return
      }
      // The outbox holds the only copy from here on.
      Self.delete(finished)
      await follow(entry, on: outbox)
    }

    /// Shows `entry`'s upload on the strip ("Sending…", "Queued", "Sent to …") and starts it.
    private func follow(_ entry: Outbox.Entry, on outbox: Outbox) async {
      if let failure = entry.failure {
        await presentFailure(ScrawlStrip.Failure(failure), entry: entry.id)
        return
      }
      // Unless a new recording took the strip over while this one was being written.
      if uploadStatus == .sending {
        shownEntry = entry.id
        sendProgress = nil
        isSendStalled = false
        delivery = DeliveryTracker(entry: entry.id, pathSatisfied: pathSatisfied, now: Date())
      }
      outbox.kick(force: true)
      refreshDelivery()
    }

    /// "Sent to …", or the two-line "On your Mac" (the recording is safe there, so it's a success
    /// too), which stays up a little longer to be read.
    private func presentSent(_ reply: IngestReply) async {
      statusID += 1
      let id = statusID
      // This one says it for any earlier upload that landed meanwhile.
      pendingSent = nil
      uploadStatus = .sent(reply)
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      try? await Task.sleep(for: .milliseconds(reply.isDelivered ? 2200 : 3500))
      guard statusID == id, case .sent = uploadStatus else { return }
      uploadStatus = .idle
      await hideOverlayAfterExit()
    }

    /// Offline, or waiting to retry: calm, with a success haptic (the recording is safe on the
    /// phone), then the strip leaves. While it's up, a retry starting turns it back into
    /// "Sending…"; when the upload lands, "Sent to …" shows whether or not it's still up.
    private func presentQueued() {
      statusID += 1
      let id = statusID
      uploadStatus = .queued
      UINotificationFeedbackGenerator().notificationOccurred(.success)
      Task { [weak self] in
        try? await Task.sleep(for: .milliseconds(3500))
        guard let self, self.statusID == id, self.uploadStatus == .queued else { return }
        self.shownEntry = nil
        self.uploadStatus = .idle
        await self.hideOverlayAfterExit()
      }
    }

    /// The failed bubble, with Share (of a LAN `recording`, or of an outbox `entry`).
    private func presentFailure(_ failure: ScrawlStrip.Failure, recording: Recording? = nil, entry: String? = nil) async {
      statusID += 1
      let id = statusID
      deliveryFailure = failure
      failedRecording = recording
      failedEntry = entry
      showOverlay()
      uploadStatus = .failed
      UINotificationFeedbackGenerator().notificationOccurred(.warning)
      try? await Task.sleep(for: .seconds(8))
      guard statusID == id, uploadStatus == .failed else { return }
      uploadStatus = .idle
      failedRecording = nil
      failedEntry = nil
      await hideOverlayAfterExit()
    }

    /// The failed bubble's Share button.
    func shareFailedRecording() {
      let recording = failedRecording
      let entry = failedEntry
      guard recording != nil || entry != nil else { return }
      failedRecording = nil
      failedEntry = nil
      uploadStatus = .idle
      Task { await self.hideOverlayAfterExit() }
      guard sharesWhenDone else { return }
      if let recording {
        Self.share([recording.movie, recording.timeline])
      } else if let entry, let outbox {
        // The payload stays queued; the share sheet gets temporary copies of its files, or the
        // bundle itself when it was built on the phone (it stays in `directory`).
        Task {
          if let name = await outbox.entries().first(where: { $0.id == entry })?.bundle {
            Self.share([Self.directory.appending(path: name, directoryHint: .isDirectory)])
            return
          }
          let folder = URL.temporaryDirectory.appending(path: "ScrawlShare", directoryHint: .isDirectory)
          guard let files = try? await outbox.extract(id: entry, into: folder) else { return }
          Self.share([files.movie, files.timeline]) {
            try? FileManager.default.removeItem(at: files.movie)
            try? FileManager.default.removeItem(at: files.timeline)
          }
        }
      }
    }

    /// Checks each Scrawl address (Macs and the HTTPS endpoint alike) and remembers the first that
    /// answers. The HTTPS endpoint answering 401 means the link was revoked: the strip says so,
    /// the recording carries on, and it's kept in the outbox as failed until the link is renewed.
    private func findMac() async {
      reachableURL = nil
      let urls = serverURLs
      guard let bearerToken = token?.trimmingCharacters(in: .whitespacesAndNewlines),
            !bearerToken.isEmpty
      else { return }
      let (reachable, revoked) = await withTaskGroup(of: (URL, Int?).self) { group in
        for base in urls {
          group.addTask {
            var request = URLRequest(url: base.appending(path: "health"))
            request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
            request.timeoutInterval = 4
            let response = try? await URLSession.shared.data(for: request).1
            return (base, (response as? HTTPURLResponse)?.statusCode)
          }
        }
        var revoked = false
        for await (base, status) in group {
          if status == 200 {
            group.cancelAll()
            return (Optional(base), false)
          }
          if status == 401 || status == 403, base.scheme?.lowercased() == "https" { revoked = true }
        }
        return (nil, revoked)
      }
      reachableURL = reachable
      guard hostedURL != nil else { return }
      if reachable?.scheme?.lowercased() == "https" {
        pairingRevoked = false
      } else if revoked {
        pairingRevoked = true
        if isRecording || isStarting { showNotice(.linkExpired, for: .seconds(5)) }
      }
    }

    /// `<stamp>.delivery` beside `<stamp>.sent`: the Mac's reply over USB.
    private nonisolated static func deliveryMarker(for sentMarker: URL) -> URL {
      sentMarker.deletingPathExtension().appendingPathExtension("delivery")
    }

    /// Uploads to a Mac, which answers within `replyWait` seconds (`X-Scrawl-Wait`) of the upload
    /// landing; the request's idle timeout leaves room for that.
    private nonisolated static func postIngest(base: URL, payload: Data, token: String, replyWait: Int) async -> IngestReply? {
      let endpoint = base.appending(path: "ingest")
      var request = URLRequest(url: endpoint)
      request.httpMethod = "POST"
      request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
      request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
      request.setValue(String(replyWait), forHTTPHeaderField: "X-Scrawl-Wait")
      request.timeoutInterval = TimeInterval(replyWait + 15)
      do {
        let (data, response) = try await URLSession.shared.upload(for: request, from: payload)
        guard let http = response as? HTTPURLResponse, (200 ..< 300).contains(http.statusCode) else {
          return nil
        }
        return IngestReply(body: data)
      } catch {
        return nil
      }
    }

    private func showOverlay() {
      guard let scene = Self.activeScene else { return }
      startupTrace?.mark("sceneFound")
      if overlay == nil {
        overlay = OverlayWindow(windowScene: scene, scrawl: self)
        startupTrace?.mark("overlayCreated")
      }
      overlay?.present(trace: startupTrace)
      startupTrace?.mark("overlayShown")
    }

    private func hideOverlay() {
      preparedAt = nil
      overlay?.dismiss()
      overlay = nil
    }

    /// Hides the overlay once the strip has animated away (it leaves as soon as nothing is left to
    /// show), unless a new recording or status took over in the meantime.
    private func hideOverlayAfterExit() async {
      try? await Task.sleep(for: StripMotion.overlayLinger)
      guard !isRecording, !isStarting, uploadStatus == .idle, !isDiscardPending else { return }
      if pendingSent != nil, UIApplication.shared.applicationState == .active {
        // An upload landed meanwhile: its "Sent to …" comes back in the same window.
        showPendingSent()
        return
      }
      hideOverlay()
    }

    static var activeScene: UIWindowScene? {
      UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        .first { $0.activationState == .foregroundActive } ?? UIApplication.shared.connectedScenes.first as? UIWindowScene
    }

    private static func share(_ files: [URL], completion: (@MainActor () -> Void)? = nil) {
      guard let scene = activeScene,
            let root = scene.windows.first(where: { $0.isKeyWindow && !($0 is OverlayWindow) })?.rootViewController
      else {
        completion?()
        return
      }
      var top = root
      while let presented = top.presentedViewController { top = presented }
      let sheet = UIActivityViewController(activityItems: files, applicationActivities: nil)
      sheet.popoverPresentationController?.sourceView = top.view
      sheet.completionWithItemsHandler = { _, _, _, _ in
        MainActor.assumeIsolated { completion?() }
      }
      top.present(sheet, animated: true)
    }

    private static func round(_ value: Double) -> Double { (value * 1000).rounded() / 1000 }
  }

  /// Whether the network came back, from `NWPathMonitor` updates (which arrive on one queue).
  private final class Reachability: @unchecked Sendable {
    private var wasSatisfied: Bool?

    /// True when `satisfied` follows an unsatisfied path (not for the first update).
    func update(_ satisfied: Bool) -> Bool {
      defer { wasSatisfied = satisfied }
      return satisfied && wasSatisfied == false
    }
  }

  /// Where a swipe began over a scroll view (`settleSwipe`), and how far the finger went each way.
  private struct SwipeStart {
    let scroll: WeakScrollView
    let offset: CGPoint
    let point: CGPoint
    var farthest = CGVector.zero
    var travel: CGFloat = 0

    mutating func reach(_ to: CGPoint) {
      let (dx, dy) = (to.x - point.x, to.y - point.y)
      farthest.dx = max(farthest.dx, abs(dx))
      farthest.dy = max(farthest.dy, abs(dy))
      travel = max(travel, hypot(dx, dy))
    }
  }

  private struct WeakScrollView {
    weak var value: UIScrollView?

    init(_ value: UIScrollView) {
      self.value = value
    }
  }
#endif
