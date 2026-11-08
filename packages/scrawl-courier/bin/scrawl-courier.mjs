#!/usr/bin/env node
// scrawl-courier: delivers new Scrawl recordings from your scrawl-server to an agent.
//   SCRAWL_TOKEN=… scrawl-courier [--config courier.json] [--once] [--backlog]
// courier.json: { "server": "https://…workers.dev", "folder": "~/Scrawl/Inbox", "every": 10,
//                 "handoff": "folder" | { "command": [...] } | { "opencode": {} } }
// See README.md.
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { parseArgs } from "node:util";

import { createCourier } from "../src/courier.js";

const { values } = parseArgs({
  options: {
    config: { type: "string", default: "courier.json" },
    once: { type: "boolean", default: false },
    backlog: { type: "boolean", default: false },
  },
});

let config;
try {
  config = JSON.parse(await readFile(values.config, "utf8"));
} catch (error) {
  console.error(`scrawl-courier: couldn't read ${values.config}: ${error.message}`);
  process.exit(1);
}
const folder = String(config.folder ?? "~/Scrawl/Inbox").replace(/^~(?=\/|$)/, homedir());
const every = Math.max(5, Number(config.every ?? 10));

let courier;
try {
  courier = createCourier({
    server: config.server,
    token: process.env.SCRAWL_TOKEN,
    folder,
    handoff: config.handoff ?? "folder",
    backlog: values.backlog,
  });
} catch (error) {
  console.error(`scrawl-courier: ${error.message}`);
  process.exit(1);
}

console.log(`scrawl-courier: checking ${config.server} every ${every} s, into ${folder}. Ctrl-C stops it.`);
for (;;) {
  try {
    await courier.poll();
  } catch (error) {
    console.error(`scrawl-courier: ${error.message}`);
    if (values.once) process.exit(1);
  }
  if (values.once) break;
  await new Promise((resolve) => setTimeout(resolve, every * 1000));
}
