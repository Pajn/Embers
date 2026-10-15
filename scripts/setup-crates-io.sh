#!/usr/bin/env bash
# Prepares every publishable workspace crate for the release workflow's trusted publishing:
#
#   1. A crate that is not on crates.io yet gets a yanked 0.0.0 placeholder, since a trusted
#      publisher can only be added to a crate that exists.
#   2. The crate gets a trusted publisher for .github/workflows/release.yml in the `crates.io`
#      environment, unless it already has one.
#   3. The crate is set to accept trusted publishing only, so API tokens can no longer publish it.
#
# Crates that are already set up are left alone, so this is safe to rerun after adding a crate.
# Without --execute it only reads from crates.io and prints what it would change.
#
# Requires cargo, curl and jq, and a crates.io API token with the publish-new, yank and
# trusted-publishing scopes for the workspace crates: CARGO_REGISTRY_TOKEN, or else the token saved
# by `cargo login`. See docs/releasing.md.
set -euo pipefail

workflow=release.yml
environment=crates.io
api=https://crates.io/api/v1
user_agent="embers-setup-crates-io (https://github.com/Pajn/Embers)"

execute=false
case "${1:-}" in
  --execute) execute=true ;;
  "") ;;
  *)
    echo "usage: $0 [--execute]" >&2
    exit 2
    ;;
esac

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  # Fall back to the token `cargo login` saved, so cargo and the API calls below use the same one.
  credentials="${CARGO_HOME:-$HOME/.cargo}/credentials.toml"
  if [[ -f "$credentials" ]]; then
    CARGO_REGISTRY_TOKEN="$(awk '
      /^[[:space:]]*\[/ { in_registry = ($0 ~ /^[[:space:]]*\[registry\][[:space:]]*$/) }
      in_registry && /^[[:space:]]*token[[:space:]]*=/ {
        sub(/^[^=]*=[[:space:]]*"/, ""); sub(/"[[:space:]]*$/, ""); print; exit
      }' "$credentials")"
  fi
fi
if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "error: no crates.io token; run \`cargo login\` or set CARGO_REGISTRY_TOKEN (see docs/releasing.md)" >&2
  exit 1
fi
export CARGO_REGISTRY_TOKEN

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

metadata="$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml")"
# `publish = false` shows up as an empty registry list.
crates="$(jq -r '.packages[] | select(.publish != []) | .name' <<<"$metadata" | sort)"

repository="$(jq -r '[.packages[].repository] | unique | if length == 1 then .[0] else error("workspace crates disagree on repository") end' <<<"$metadata")"
repo_path="${repository#https://github.com/}"
repo_path="${repo_path%/}"
repo_path="${repo_path%.git}"
owner="${repo_path%%/*}"
repo="${repo_path#*/}"
if [[ "$repo_path" == "$repository" || -z "$owner" || -z "$repo" || "$repo" == */* ]]; then
  echo "error: repository ${repository} is not a GitHub repository URL" >&2
  exit 1
fi

# Calls the crates.io API and prints the response body; fails on any non-2xx status.
call() {
  local method="$1" path="$2"
  shift 2
  curl -fsS --retry 3 -X "$method" -A "$user_agent" \
    -H "Authorization: ${CARGO_REGISTRY_TOKEN}" \
    -H "Content-Type: application/json" \
    "$@" "${api}${path}"
}

# Prints the HTTP status of GET /crates/<crate>, leaving the body in $work/crate.json.
crate_status() {
  curl -sS --retry 3 -A "$user_agent" -o "$work/crate.json" -w '%{http_code}' "${api}/crates/$1"
}

# Runs a change, or only describes it in a dry run.
change() {
  local description="$1"
  shift
  if "$execute"; then
    echo "  ${description}"
    "$@"
  else
    echo "  would ${description}"
  fi
}

publish_placeholder() {
  local crate="$1" license dir
  license="$(jq -r --arg crate "$crate" '.packages[] | select(.name == $crate) | .license' <<<"$metadata")"
  dir="$work/$crate"
  mkdir -p "$dir/src"
  : >"$dir/src/lib.rs"
  cat >"$dir/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.0.0"
edition = "2024"
license = "$license"
repository = "$repository"
description = "Placeholder reserving the name. Releases are published from $repository."

[workspace]
EOF
  cargo publish --manifest-path "$dir/Cargo.toml" --no-verify
  # The registry can take a moment to show a new version, and a rerun skips a
  # crate that exists, so a failed yank would leave the placeholder up.
  local attempt
  for attempt in 1 2 3 4 5 6; do
    cargo yank --version 0.0.0 "$crate" && return
    echo "yanking $crate 0.0.0 failed, retrying" >&2
    sleep $((attempt * 5))
  done
  echo "could not yank $crate 0.0.0; yank it by hand" >&2
  return 1
}

add_trusted_publisher() {
  local body
  body="$(jq -n --arg crate "$1" --arg owner "$owner" --arg repo "$repo" \
    --arg workflow "$workflow" --arg environment "$environment" \
    '{github_config: {crate: $crate, repository_owner: $owner, repository_name: $repo,
      workflow_filename: $workflow, environment: $environment}}')"
  call POST /trusted_publishing/github_configs --data "$body" >/dev/null
}

require_trusted_publishing() {
  call PATCH "/crates/$1" --data '{"crate": {"trustpub_only": true}}' >/dev/null
}

"$execute" || echo "Dry run; pass --execute to make these changes."
echo "Trusted publisher: ${owner}/${repo}, workflow ${workflow}, environment ${environment}"

for crate in $crates; do
  echo "$crate"

  status="$(crate_status "$crate")"
  case "$status" in
    200)
      exists=true
      trustpub_only="$(jq -r '.crate.trustpub_only // false' "$work/crate.json")"
      ;;
    404)
      exists=false
      trustpub_only=false
      change "publish and yank a 0.0.0 placeholder" publish_placeholder "$crate"
      ;;
    *)
      echo "error: crates.io returned HTTP ${status} for ${crate}" >&2
      exit 1
      ;;
  esac

  # The environment of a matching publisher: ours, "any" for one that allows every environment, or
  # empty when there is none.
  publisher=""
  if "$exists"; then
    configs="$(call GET "/trusted_publishing/github_configs?crate=${crate}")"
    publisher="$(jq -r --arg owner "$owner" --arg repo "$repo" \
      --arg workflow "$workflow" --arg environment "$environment" '
        [.github_configs[]
          | select((.repository_owner | ascii_downcase) == ($owner | ascii_downcase)
            and (.repository_name | ascii_downcase) == ($repo | ascii_downcase)
            and .workflow_filename == $workflow
            and (.environment == $environment or .environment == null))
          | .environment]
        | if any(.[]; . == $environment) then $environment elif length > 0 then "any" else "" end
      ' <<<"$configs")"
  fi

  case "$publisher" in
    "$environment") echo "  trusted publisher already configured" ;;
    any) echo "  trusted publisher already configured, for any environment" ;;
    *) change "add the trusted publisher" add_trusted_publisher "$crate" ;;
  esac

  if [[ "$trustpub_only" == true ]]; then
    echo "  already requires trusted publishing"
  else
    change "require trusted publishing" require_trusted_publishing "$crate"
  fi
done
