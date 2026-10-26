import Foundation

/// Waiting on a framework's callback without waiting forever: speech recognition has no deadline
/// of its own, and right after a recording ends iOS can leave a frame request unanswered.
enum Deadline {
  /// The first answer `start`'s work gives, or nil when `limit` passes first. `start` begins the
  /// work, hands it `reply` (only the first call counts) and returns what stops it, which runs
  /// when `limit` passes first.
  static func answer<T>(
    within limit: Duration,
    _ start: (_ reply: @escaping @Sendable (T) -> Void) -> @Sendable () -> Void
  ) async -> T? {
    let once = Once()
    return await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
      let stop = start { value in
        if once.claim() { continuation.resume(returning: value) }
      }
      Task {
        try? await Task.sleep(for: limit)
        if once.claim() {
          stop()
          continuation.resume(returning: nil)
        }
      }
    }
  }

  private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func claim() -> Bool { lock.withLock { defer { done = true }; return !done } }
  }
}
