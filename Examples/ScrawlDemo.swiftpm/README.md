# Scrawl demo

A small iPhone app with ScrawlKit in it. It builds each recording into a `.scrawl` bundle on the
phone (`Scrawl.shared.compiles = .onDevice`), with no Mac running Scrawl.

## Run it (about 10 minutes the first time)

1. On the Mac, from the repository's folder, build the compiler for iPhone once (about 5
   minutes):

   ```sh
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin x86_64-apple-darwin
   scripts/build-xcframework.sh
   ```

   It makes `target/ScrawlCore.xcframework`, which ScrawlKit picks up on its own. If Xcode had the
   demo open already, choose **File → Packages → Reset Package Caches** so it notices.
2. Open `Examples/ScrawlDemo.swiftpm` in Xcode (double-click it, or `open Examples/ScrawlDemo.swiftpm`).
3. In Xcode, choose the ScrawlDemo app, then **Signing & Capabilities**, and pick your Team (a
   free Apple ID works).
4. Plug in the iPhone, pick it as the run destination at the top, and press **Run** (⌘R). The
   first time, the iPhone asks you to trust the developer: **Settings → General → VPN & Device
   Management**.

The repository's folder must be named `scrawl`: the demo refers to the package by that name.

## What to try (about 15 minutes)

1. **A short recording.** Three-finger long press, say "this button should say Save", tap the
   counter, then three-finger long press again. iOS asks for the microphone, and then for speech
   recognition, once each. The strip says "Building…", then "Saved on this iPhone", and the share
   sheet offers the bundle. AirDrop it to the Mac and open `summary.md`: the words should be
   there, with the tap and "Counter tapped".
2. **Ink.** Record again and turn on Draw to circle the text field. The frames should show the red
   circle.
3. **A long one.** Record for about 2 minutes while scrolling and tapping. Note how long
   "Building…" takes. If the app is killed, Xcode's console says so.
4. **No permission.** In Settings → ScrawlDemo, turn off Speech Recognition, then record again.
   The bundle should still build, and `summary.md` should say why there's no transcript.
5. **Leaving mid-build.** Stop a recording by going to the Home Screen. Come back after a
   minute. The bundle should be in the list. If iOS ended the build, it's built again the next
   time the app starts.

The bundles also show in the app's list, each with Share.
