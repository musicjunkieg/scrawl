# Smart routing

Read this before changing `crates/scrawl-route/src/smart.rs` or `src/routing/look.rs`. The ladder that runs when the experiment is off is described in `docs/iphone-and-routing.md`, and the words used here are defined in `CONTEXT.md`.

Smart routing is an experiment, on with `"experiments": {"smartRouting": true}` in `~/.config/scrawl/config.json` (read through `handoff::experiment`). With it off the app calls `routing::route_capture` exactly as before. With it on, a Mac Screen or Window capture is routed in three steps.

1. `look::look_live` runs while the bundle is built. It reads the browser tab in front (or captured) and turns it into clues: the page's `/sdk.js` registration, and the folder of the process serving its port when the tab is on this Mac. "On this Mac" means a loopback host or one of this Mac's own names (`hostname`, its `.local` name, its Tailscale name), whose port is mapped through `tailscale serve status --json`. It reads Ghostty's focused pane (folder and title) when Ghostty is captured or in front. Then it reads the open root sessions: the newest `SESSION_PAGES` pages, the newest page in each folder a clue names, and each session a clue names.
2. Once the bundle exists, `look::evidence_for` adds clues from it (the window title minus the app's own name, the captured app's name unless it's a browser, terminal, editor or system app, the transcript, and for OpenCode Desktop the session viewed last) and the recorded destination as an address. `smart::decide(&evidence, Policy::Weigh)` returns a judgement. `routing::judge_capture` saves the evidence and decision as `routing.json` in the bundle and logs one line.
3. `ScrawlCoordinator::act_on_route` acts on it after the capture is copied. A send shows "Sending to …" for `SMART_SEND_UNDO` (3 s), during which ✕ or Paste on the Completed island cancels it. Then the destination is written to `scrawl.json` and `summary.md` and sent through `send_completed` (and so `delivery::deliver`, with its retries). An offer only names the suggestions in the review window's status.

## Scoring

A candidate's score is the sum, over sources, of the best points any one clue of that source gives it, plus recency if that sum is above zero. The points are in `Source::points` and the thresholds are `SEND_AT` (95), `LEAD` (15) and `OFFER_AT` (40).

- Page (`data-session`: 100, `data-directory`: 95) and Port (95) send on their own. Terminal (70), Document (60, not gathered yet), Viewed (60), App (50), Title (40) and Speech (25) don't.
- A folder clue gives full points to the sessions closest to its folder (on the line from the project root to it, or inside it), 15 fewer per extra folder for sessions further along that line, and half to a session elsewhere in the project or in another checkout of the same repository (`look::repo_of` follows a worktree's `commondir`).
- A word clue's points are divided by the number of candidates the word names.
- Recency: 20 within 10 minutes of the capture's end, 12 within an hour, 5 within a day, counting the session's update or view times up to a minute after the end.

## Laws

The tests in `smart.rs` check each one. An address always wins, and a choice beats a recorded destination. A send needs `SEND_AT` and `LEAD`. `decide` is pure, so the same evidence gives the same decision, in any order and with clues repeated. Recency never puts a session forward on its own. Adding support for the leader never demotes it. The home folder and `/` are never projects. One medium clue (the viewed session, a terminal folder, the app name) can't send without a second that agrees. `look::tests::the_ladder_policy_reproduces_route` checks that `Policy::Ladder` over the same hooks decides what `routing::route` does on a table of cases.

## Tools

- `scrawl route --explain <bundle|latest> [--url URL]` prints the evidence, the sessions weighed with their points, the decision, and what the ladder does. It replays `routing.json` when the bundle has one. Otherwise it gathers what it can now (sessions as they are today, so recency and the viewed session are only approximate), and `--url` stands in for the tab.
- Tests swap the outside steps through `look::Senses` (which holds `routing::Hooks`), and never post to a session.
