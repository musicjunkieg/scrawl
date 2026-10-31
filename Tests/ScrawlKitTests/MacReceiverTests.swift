#if os(macOS)
  @testable import ScrawlKit
  import Testing

  struct MacReceiverTests {
    @Test func aSetupLinkGivesTheReceiver() throws {
      let link = "scrawldemo://scrawl?url=https%3A%2F%2Fscrawl-server.example.workers.dev&token=up-123%0A"
      let receiver = try #require(Scrawl.Receiver(setupLink: "  \(link)\n"))
      #expect(receiver.url.absoluteString == "https://scrawl-server.example.workers.dev")
      #expect(receiver.token == "up-123")
    }

    @Test func anythingElseIsNoReceiver() {
      for text in [
        "",
        "https://scrawl-server.example.workers.dev",
        "scrawldemo://other?url=https%3A%2F%2Fa.example&token=t",
        "scrawldemo://scrawl?url=https%3A%2F%2Fa.example",
        "scrawldemo://scrawl?url=https%3A%2F%2Fa.example&token=%20",
        "scrawldemo://scrawl?url=ftp%3A%2F%2Fa.example&token=t",
        "scrawldemo://scrawl?token=t",
      ] {
        #expect(Scrawl.Receiver(setupLink: text) == nil, "\(text)")
      }
    }
  }
#endif
