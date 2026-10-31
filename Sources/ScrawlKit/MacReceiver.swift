#if os(macOS)
  import Foundation
  import Security

  extension Scrawl {
    /// A Scrawl receiver (`scrawl-server`, `docs/receiver.md`) that each bundle built here is also
    /// sent to, with its upload token. Kept in the Keychain, one per app.
    public struct Receiver: Codable, Equatable, Sendable {
      public var url: URL
      public var token: String

      /// Nil unless `url` is HTTPS (plain HTTP only to this Mac, for testing a receiver run here)
      /// and the token isn't empty: the token and the recordings would otherwise go in the clear.
      public init?(url: URL, token: String) {
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard Self.isSafe(url), !token.isEmpty else { return nil }
        self.url = url
        self.token = token
      }

      static func isSafe(_ url: URL) -> Bool {
        guard let host = url.host()?.lowercased(), !host.isEmpty else { return false }
        switch url.scheme?.lowercased() {
        case "https": return true
        case "http": return ["localhost", "127.0.0.1", "::1", "[::1]"].contains(host)
        default: return false
        }
      }

      /// From a setup link, `<scheme>://scrawl?url=<https base>&token=<upload token>`, the same
      /// link ScrawlKit takes on the iPhone. Nil for anything else.
      public init?(setupLink: String) {
        guard let components = URLComponents(string: setupLink.trimmingCharacters(in: .whitespacesAndNewlines)),
              components.host?.lowercased() == "scrawl",
              let items = components.queryItems,
              let raw = items.first(where: { $0.name == "url" })?.value,
              let url = URL(string: raw.trimmingCharacters(in: .whitespacesAndNewlines)),
              let token = items.first(where: { $0.name == "token" })?.value
        else { return nil }
        self.init(url: url, token: token)
      }

      private static let service = "dev.kitlangton.scrawl.receiver"
      private static var account: String { Bundle.main.bundleIdentifier ?? "app" }

      /// The receiver saved for this app, if any.
      static func saved() -> Receiver? {
        var result: CFTypeRef?
        let query: [String: Any] = [
          kSecClass as String: kSecClassGenericPassword,
          kSecAttrService as String: service,
          kSecAttrAccount as String: account,
          kSecReturnData as String: true,
        ]
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data
        else { return nil }
        return try? JSONDecoder().decode(Receiver.self, from: data)
      }

      /// Saves `receiver` for this app, or forgets it (nil). False when the Keychain refused, so a
      /// server the person stopped sending to could come back at the next launch.
      @discardableResult
      static func save(_ receiver: Receiver?) -> Bool {
        let item: [String: Any] = [
          kSecClass as String: kSecClassGenericPassword,
          kSecAttrService as String: service,
          kSecAttrAccount as String: account,
        ]
        let deleted = SecItemDelete(item as CFDictionary)
        guard deleted == errSecSuccess || deleted == errSecItemNotFound else {
          print("Scrawl: the Scrawl server couldn't be removed from the Keychain (\(deleted))")
          return false
        }
        guard let receiver, let data = try? JSONEncoder().encode(receiver) else { return receiver == nil }
        var adding = item
        adding[kSecValueData as String] = data
        let status = SecItemAdd(adding as CFDictionary, nil)
        guard status == errSecSuccess else {
          print("Scrawl: the Scrawl server couldn't be saved in the Keychain (\(status))")
          return false
        }
        return true
      }
    }
  }
#endif
