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
through `rust-lang/crates-io-auth-action`, pinned to a commit. No crates.io API token is stored in
the repository.

Each published crate (`embers-core`, `embers-protocol`, `embers-server`, `embers-test-support`,
`embers-client`, `embers-ghostty-sys`, `embers-ghostty-backend`, `embers-cli`) needs a trusted
publisher on crates.io, with:

- repository owner and name: this repository
- workflow filename: `release.yml`
- environment: `crates.io`

`scripts/setup-crates-io.sh` sets this up for every workspace crate that isn't marked
`publish = false`. A trusted publisher can only be added to a crate that already exists, so for a
crate that isn't on crates.io yet the script first publishes a placeholder `0.0.0` and yanks it.
It then adds the trusted publisher and sets the crate to accept trusted publishing only. Crates
that are already set up are skipped. By default it only reads from crates.io and prints what it
would change; `--execute` makes the changes:

```sh
scripts/setup-crates-io.sh            # dry run
scripts/setup-crates-io.sh --execute
```

It needs `cargo`, `curl` and `jq`, and a crates.io API token from an owner of the crates, with the
`publish-new`, `yank` and `trusted-publishing` scopes for the workspace crates. It uses
`CARGO_REGISTRY_TOKEN` if that is set, and otherwise the token saved by `cargo login`.

### Adding a crate

1. Add the crate to the `crates` list in the `publish-crates` job of
   `.github/workflows/release.yml`, after the crates it depends on.
2. Run `scripts/setup-crates-io.sh --execute` before the next release is tagged.
