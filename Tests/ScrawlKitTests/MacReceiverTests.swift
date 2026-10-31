#if os(macOS)
  import Foundation
  @testable import ScrawlKit
  import Testing

  struct MacReceiverTests {
    @Test func aSetupLinkGivesTheReceiver() throws {
      let link = "scrawldemo://scrawl?url=https%3A%2F%2Fscrawl-server.example.workers.dev&token=up-123%0A"
      let receiver = try #require(Scrawl.Receiver(setupLink: "  \(link)\n"))
      #expect(receiver.url.absoluteString == "https://scrawl-server.example.workers.dev")
      #expect(receiver.token == "up-123")
    }

    @Test func plainHTTPIsOnlyForThisMac() throws {
      let local = try #require(Scrawl.Receiver(setupLink: "x://scrawl?url=http%3A%2F%2Flocalhost%3A8787&token=t"))
      #expect(local.url.absoluteString == "http://localhost:8787")
      #expect(Scrawl.Receiver(url: URL(string: "http://127.0.0.1:8787")!, token: "t") != nil)
      #expect(Scrawl.Receiver(url: URL(string: "http://192.168.1.20:8787")!, token: "t") == nil)
      #expect(Scrawl.Receiver(url: URL(string: "https://a.example")!, token: " ") == nil)
    }

    @Test func aSavedReceiverIsCheckedAgainWhenRead() throws {
      let saved = try JSONEncoder().encode(try #require(Scrawl.Receiver(url: URL(string: "https://a.example")!, token: "t")))
      #expect(try JSONDecoder().decode(Scrawl.Receiver.self, from: saved).token == "t")
      let tampered = Data(#"{"url":"http://a.example","token":"t"}"#.utf8)
      #expect(throws: DecodingError.self) { try JSONDecoder().decode(Scrawl.Receiver.self, from: tampered) }
    }

    @Test func anythingElseIsNoReceiver() {
      for text in [
        "",
        "https://scrawl-server.example.workers.dev",
        "scrawldemo://other?url=https%3A%2F%2Fa.example&token=t",
        "scrawldemo://scrawl?url=https%3A%2F%2Fa.example",
        "scrawldemo://scrawl?url=https%3A%2F%2Fa.example&token=%20",
        "scrawldemo://scrawl?url=ftp%3A%2F%2Fa.example&token=t",
        // The token and recordings would go in the clear.
        "scrawldemo://scrawl?url=http%3A%2F%2Fscrawl-server.example.workers.dev&token=t",
        "scrawldemo://scrawl?token=t",
      ] {
        #expect(Scrawl.Receiver(setupLink: text) == nil, "\(text)")
      }
    }
  }
#endif
