// `cf` builds this Worker with Wrangler's bundler, which runs the Rust build first.
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
	build: {
		// Quoted: cmd.exe on Windows would take the ^ as an escape and drop it.
		command: 'cargo install -q "worker-build@^0.8" && worker-build --release',
	},
});
