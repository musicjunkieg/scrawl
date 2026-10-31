#if os(macOS)
  import Foundation
  import Security

  extension Scrawl {
    /// A Scrawl receiver (`scrawl-server`, `docs/receiver.md`) that each bundle built here is also
    /// sent to, with its upload token. Kept in the Keychain, one per app.
    public struct Receiver: Codable, Equatable, Sendable {
      public var url: URL
      public var token: String

      public init(url: URL, token: String) {
        self.url = url
        self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
      }

      /// From a setup link, `<scheme>://scrawl?url=<https base>&token=<upload token>`, the same
      /// link ScrawlKit takes on the iPhone. Nil for anything else.
      public init?(setupLink: String) {
        guard let components = URLComponents(string: setupLink.trimmingCharacters(in: .whitespacesAndNewlines)),
              components.host?.lowercased() == "scrawl",
              let items = components.queryItems,
              let raw = items.first(where: { $0.name == "url" })?.value,
              let url = URL(string: raw.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http",
              url.host() != nil,
              let token = items.first(where: { $0.name == "token" })?.value,
              !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
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

      /// Saves `receiver` for this app, or forgets it (nil).
      static func save(_ receiver: Receiver?) {
        let item: [String: Any] = [
          kSecClass as String: kSecClassGenericPassword,
          kSecAttrService as String: service,
          kSecAttrAccount as String: account,
        ]
        SecItemDelete(item as CFDictionary)
        guard let receiver, let data = try? JSONEncoder().encode(receiver) else { return }
        var adding = item
        adding[kSecValueData as String] = data
        let status = SecItemAdd(adding as CFDictionary, nil)
        if status != errSecSuccess {
          print("Scrawl: the Scrawl server couldn't be saved in the Keychain (\(status))")
        }
      }
    }
  }
#endif
