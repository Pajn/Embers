# Releasing

All crates in the workspace share one version. A release is a commit that bumps it and a
`v<version>` tag on `main`; pushing the tag runs `.github/workflows/release.yml`, which validates
the workspace, publishes the crates to crates.io, creates the GitHub release and attaches the
`embers-cli` binaries.

## Cutting a release

Releases are cut with [cargo-release](https://github.com/crate-ci/cargo-release), configured in
`release.toml`. From an up-to-date `main`:

```sh
cargo release minor            # dry run: shows the version bump, commit, tag and push
cargo release minor --execute  # bump, commit `v<version>`, tag `v<version>` and push
```

Use `patch`, `minor` or `major` as the level, or an explicit version. cargo-release only bumps,
commits, tags and pushes; publishing to crates.io happens in CI once the tag is pushed.

## Publishing to crates.io

The release workflow publishes with
[trusted publishing](https://crates.io/docs/trusted-publishing): the `publish-crates` job runs in
the `crates.io` GitHub environment and exchanges its OIDC token for a short-lived crates.io token
through `rust-lang/crates-io-auth-action`. No crates.io API token is stored in the repository.

Each published crate (`embers-core`, `embers-protocol`, `embers-server`, `embers-test-support`,
`embers-client`, `embers-cli`) needs a trusted publisher configured once in its settings on
crates.io, with:

- repository owner and name: this repository
- workflow filename: `release.yml`
- environment: `crates.io`

A new crate has to be published once by hand before a trusted publisher can be added to it.
