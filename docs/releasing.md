# Releasing

A release puts Scrawl's libraries where other apps can install them without building anything:

| What | Where | How an app gets it |
|---|---|---|
| `ScrawlCore.xcframework`, the compiler ScrawlKit links (`crates/scrawl-ffi`) | GitHub Releases, tag `scrawlcore-<version>` | Swift Package Manager downloads it through `Package.swift`'s `coreRelease` |
| `scrawl-bundle`, `scrawl-capture`, `scrawl-core`, `scrawl-route`, `scrawl-wasm` | crates.io | `cargo add scrawl-core` |
| `scrawl-web` | npm | `npm install scrawl-web` |
| ScrawlKit itself | this repository, tag `<version>` | `.package(url: "https://github.com/musicjunkieg/scrawl", from: "<version>")` |

The `Release` workflow (`.github/workflows/release.yml`) does the first three. It's run by hand, and each part can be switched off.

## Once: the two tokens

crates.io and npm each need a token saved in the repository as a secret. GitHub keeps secrets hidden, even from people who can see the repository.

1. **crates.io** (about 5 minutes):
   - Sign in at https://crates.io with GitHub, and verify your email under Account Settings (crates.io won't publish without it).
   - Account Settings → API Tokens → New Token. Name it `scrawl release`, and tick the scopes `publish-new` and `publish-update`. Copy the token; it's shown once.
   - On GitHub: the repository's Settings → Secrets and variables → Actions → New repository secret. Name `CARGO_REGISTRY_TOKEN`, value the token.
2. **npm** (about 5 minutes):
   - Create an account at https://www.npmjs.com and turn on two-factor authentication.
   - Your avatar → Access Tokens → Generate New Token → Granular Access Token. Name it `scrawl release`, allow it to bypass two-factor authentication, and give it read and write access to all packages, since `scrawl-web` doesn't exist yet. Copy the token.
   - Add it as a repository secret named `NPM_TOKEN`.

## Each release

1. Check that every version says the release's number: the `version` in `Cargo.toml` and in each `crates/*/Cargo.toml` (and in their `scrawl-*` path dependencies), and in `packages/scrawl-web/package.json`. The workflow refuses to run if `Cargo.toml` or `package.json` disagree with what you type.
2. On GitHub: Actions → Release → Run workflow. Type the version (`0.3.5`) and run it. It takes about 10 minutes.
3. The ScrawlCore job's summary and the new `scrawlcore-<version>` release both show a `checksum`. Put the release's URL and that checksum in `Package.swift`'s `coreRelease`, and merge that.
4. Tag the merged commit `<version>` (Releases → Draft a new release → new tag `0.3.5` on `main`). That tag is the version Swift Package Manager installs.

crates.io and npm never take the same version twice. If the crates job stops partway, running it again fails at the first crate already published; publish the remaining ones by hand with `cargo publish -p <name>`, in the order the workflow lists them.
