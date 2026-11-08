# scrawl-courier

Picks up each new Scrawl recording from your [`scrawl-server`](../../workers/scrawl-server/README.md)
and hands it to an agent: any CLI agent (a command), an OpenCode session, or a folder that a tool
watches. It runs on the computer where your agent runs, and asks the receiver for new recordings
every few seconds (a laptop can't be reached from the cloud, so it asks rather than being rung).

```
 your scrawl-server ──GET /bundles──> scrawl-courier ──> <folder>/<id>.scrawl/ ──> hand-off
                                      (every 10 s)                                  command | opencode | folder
```

## Run it

It needs Node 22 or newer and the receiver's **read** token (`SCRAWL_TOKEN`; the upload token
can't list recordings). Write a `courier.json`:

```json
{
  "server": "https://scrawl-server.you.workers.dev",
  "folder": "~/Scrawl/Inbox",
  "every": 10,
  "handoff": { "command": ["claude", "-p", "Read the Scrawl recording in {dir}: start with {summary} and the contact sheet {contactSheet}, then fix what it reports."] }
}
```

Then, from the folder with `courier.json` (here reading the token from the receiver's secrets file):

```sh
set -a && source path/to/workers/scrawl-server/.secrets.env && set +a
node path/to/packages/scrawl-courier/bin/scrawl-courier.mjs
```

The first run notes the recordings already on the receiver and leaves them (`--backlog` delivers
them too); after that each new one is downloaded whole into `<folder>/<id>.scrawl/` and handed
off, oldest first. What's been delivered is kept in `<folder>/.courier.json`, so a restart
delivers nothing twice. A hand-off that fails is tried again on the next look, three times in
all, then set aside and logged; the download stays in the folder. `--once` looks once and exits
(for cron or launchd).

## Hand-offs

- **`"folder"`** (the default): nothing more. The recording waits in the folder.
- **`{ "command": [program, ...args] }`**: runs any CLI agent (Claude Code's `claude -p`,
  `codex exec`, `aider --message`, a script of your own). `{id}`, `{dir}`, `{summary}`,
  `{contactSheet}`, `{scrawlJson}` and `{app}` in the arguments are filled in, and the same are in
  `SCRAWL_ID`, `SCRAWL_DIR`, `SCRAWL_SUMMARY`, `SCRAWL_CONTACT_SHEET`, `SCRAWL_JSON` and
  `SCRAWL_APP`. It runs in the bundle's folder (`cwd` to change it) without a shell, so text from
  a recording never becomes a command: don't wrap it in `sh -c`. A zero exit counts as delivered;
  `timeoutSeconds` stops one that runs too long. `claude -p` starts a new Claude Code session for
  each recording; to look at one in a session you already have open, use the receiver's MCP
  tools (`get_scrawl`) instead.
- **`{ "opencode": {} }`**: a prompt into an OpenCode session, as the Scrawl Mac app sends one
  (`opencode2 api post /api/session/<id>/prompt`, the summary as text and the contact sheet
  attached): the open root session whose title matches the recording's app, else the newest.
  `"session": "ses_…"` picks one; `"bin"` (or `OPENCODE_BIN`) names the OpenCode command if it
  isn't `opencode2` on your `PATH`.

## Who can send you a recording

Anyone with your app can send one: the upload token ships inside it. So a recording is a report
to check, not instructions to follow, and someone could narrate instructions at your agent. Run
the agent with the permissions you'd give a stranger's bug report: no deploys or secrets, and
asking before it runs commands. The OpenCode prompt says so to the agent too.
