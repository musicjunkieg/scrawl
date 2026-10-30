// The receiver on Cloudflare Workers, deployed with `cf` (Cloudflare's CLI). README.md here has
// the steps; wrangler.config.ts holds the Rust build.
import { bindings, defineConfig } from "cf/config";

export default defineConfig({
	worker: {
		name: "scrawl-server",
		compatibilityDate: "2026-10-01",
		entrypoint: "build/worker/shim.mjs",
		env: {
			// Where bundles are kept: `cf deploy` makes the bucket the first time.
			BUNDLES: bindings.r2({ name: "scrawl-bundles" }),
			// The largest upload, in bytes (40 MB). A Worker has 128 MB of memory and holds an upload
			// about twice while unpacking it, so raise this with care (Workers itself takes up to 100 MB).
			MAX_UPLOAD_BYTES: bindings.text("41943040"),
		},
	},
});
