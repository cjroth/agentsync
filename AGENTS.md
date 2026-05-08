* Always follow test-driven development with heavy use of e2e tests. End-to-end tests are how you know it actually works.
* After every change:
  * Ensure that you have good test coverage and all tests are passing.
  * Ensure that cargo compiles with no warnings or errors.
  * If you touch `agentsync-core` or `agentsync-wasm`, also run `cargo check -p agentsync-core -p agentsync-wasm --target wasm32-unknown-unknown` — the wasm boundary is gated by `cfg(target_arch = "wasm32")` and breaks silently if you add a tokio/notify/rustls dep without conditionalizing it.
  * If you touch `sdks/typescript/src/**` or the wasm crate, run `bun run build && bun run lint && bun test` from `sdks/typescript/` to verify the JS surface still builds, lints under biome, and unit-tests pass. The e2e tests need a real `agentsync` binary on PATH (or `AGENTSYNC_BIN` env var); CI builds it in the same job.
  * Self review your code
  * Run /simplify to simplify code
* Supply-chain hygiene when adding or bumping deps:
  * **GitHub Actions** are SHA-pinned with a `# vX.Y.Z` comment. To bump, run `git ls-remote https://github.com/<owner>/<repo> refs/tags/<tag>^{}` and update both the SHA and the comment in lockstep.
  * **Bun (npm)** packages are gated on `minimumReleaseAge = 604800` (7d) via `sdks/typescript/bunfig.toml`. To intentionally pull a fresher package, add it to `minimumReleaseAgeExcludes` in the same file with a comment naming the CVE / advisory.
  * **Cargo** has no native minimum-age yet (rust-lang/cargo#15973). We compensate with `Cargo.lock` + `cargo build --locked` everywhere (CI workflows enforce it) and `cargo deny check advisories bans sources` as a hard gate. Reviewers bumping `Cargo.lock` should manually check that any new transitive version has been on crates.io for at least 7 days (visible on the crate page).
