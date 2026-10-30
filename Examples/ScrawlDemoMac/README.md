# Scrawl demo for the Mac

A small Mac app with ScrawlKit in it. It records its own window with ScreenCaptureKit and builds
each recording into a `.scrawl` bundle on this Mac (`Scrawl.shared.compiles = .onDevice`, the
default once ScrawlCore is built).

## Run it (about 10 minutes the first time)

1. From the repository's folder, build the compiler once (about 5 minutes; skip it if you built
   it for the iPhone demo):

   ```sh
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin x86_64-apple-darwin
   scripts/build-xcframework.sh
   ```

2. Build and open the demo:

   ```sh
   Examples/ScrawlDemoMac/run.sh
   ```

   It builds the app, wraps it as `Examples/ScrawlDemoMac/.build/ScrawlDemo.app` and opens it, so
   macOS asks for ScrawlDemo's own permissions. It signs the app with your "Apple Development"
   certificate (Xcode › Settings › Accounts › Manage Certificates › + › Apple Development, free),
   so the permissions stay across rebuilds; `SCRAWL_SIGN="…"` picks another identity. Without one
   it signs ad hoc, and each rebuild is a new app to macOS, which asks again. Permissions granted
   to earlier ad-hoc builds can be cleared with `tccutil reset ScreenCapture dev.scrawl.demo.mac`
   (and `Microphone`).

## What to try (about 15 minutes)

1. **A short recording.** Press ⌃⌥⇧S. macOS asks to let ScrawlDemo record the screen: allow it
   in System Settings › Privacy & Security › Screen & System Audio Recording, quit the demo and
   run it again (macOS needs the restart), then press ⌃⌥⇧S again. Allow the microphone. Say "this
   button should say Save", click the counter, then press ⌃⌥⇧S again. The bar at the bottom says
   "Building the bundle…", macOS asks for speech recognition once, and the bundle shows in the
   window's list. **Open summary** should have your words, the click on "Clicked 0 times" (the label as the
   click went down) and "Counter clicked (1)".
2. **The Help menu.** Help › Record with Scrawl starts it too; the same item stops it. Help ›
   Mark This Moment adds a marker.
3. **Ink.** Record again, hold ⇧ and drag a circle around the text field. The frames should
   show the red circle.
4. **Moving the window.** Record, drag the window somewhere else, click something, stop. The
   frames should follow the window, and the summary shouldn't have the title-bar drag.
5. **Other apps stay out.** Put another app's window half over the demo and record. The other
   app shouldn't show in the frames.
6. **A slip.** Press ⌃⌥⇧S, and as soon as the bar says "● Recording" press it again. The bar says
   "Too short to keep" and no bundle is built. (Pressed again before "● Recording" shows, it says
   "Stopped before anything was recorded" instead.)

Bundles are in `~/Library/Application Support/ScrawlKit/dev.scrawl.demo.mac/`.
