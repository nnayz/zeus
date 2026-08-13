#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <version> <dmg-path> <repo-checkout>" >&2
    exit 2
fi

version="$1"
dmg_path="$2"
repo_dir="$3"
gh_repo="${GH_REPO:-nnayz/zeus}"
gh_bin="${GH_BIN:-gh}"
cask_relative="Casks/zeus.rb"
asset_name="zeus-${version}-universal.dmg"

if ! [[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version '${version}' is not X.Y.Z" >&2
    exit 2
fi
if [[ ! -f "${dmg_path}" ]]; then
    echo "error: DMG does not exist: ${dmg_path}" >&2
    exit 1
fi
if [[ ! -d "${repo_dir}/.git" || ! -f "${repo_dir}/${cask_relative}" ]]; then
    echo "error: Zeus checkout is missing ${cask_relative}: ${repo_dir}" >&2
    exit 1
fi

local_sha="$(shasum -a 256 "${dmg_path}" | awk '{print $1}')"
published_digest="$(
    "${gh_bin}" release view "v${version}" \
        --repo "${gh_repo}" \
        --json assets \
        --jq ".assets[] | select(.name == \"${asset_name}\") | .digest"
)"
published_sha="${published_digest#sha256:}"

if ! [[ "${published_sha}" =~ ^[0-9a-f]{64}$ ]]; then
    echo "error: GitHub release v${version} has no SHA-256 digest for ${asset_name}" >&2
    exit 1
fi
if [[ "${local_sha}" != "${published_sha}" ]]; then
    cat >&2 <<EOF
error: local DMG does not match the published GitHub asset
  local:     ${local_sha}
  published: ${published_sha}

Release assets are immutable. Cut a new version instead of changing v${version}.
EOF
    exit 1
fi

if [[ -n "$(git -C "${repo_dir}" status --porcelain --untracked-files=no)" ]]; then
    echo "error: tracked changes in Zeus checkout: ${repo_dir}" >&2
    exit 1
fi

branch="$(git -C "${repo_dir}" branch --show-current)"
if [[ -z "${branch}" ]]; then
    echo "error: Zeus checkout is on a detached HEAD" >&2
    exit 1
fi
if ! upstream="$(git -C "${repo_dir}" rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null)"; then
    echo "error: Zeus branch '${branch}' has no upstream" >&2
    exit 1
fi
remote="${upstream%%/*}"
remote_branch="${upstream#*/}"

# Fetch before editing so a concurrent repository update fast-forwards cleanly
# or stops here. A non-fast-forward push below is the second race barrier.
git -C "${repo_dir}" pull --ff-only --quiet "${remote}" "${remote_branch}"

/usr/bin/sed -i '' -E \
    -e "s|^  version \".*\"$|  version \"${version}\"|" \
    -e "s|^  sha256 \".*\"$|  sha256 \"${local_sha}\"|" \
    "${repo_dir}/${cask_relative}"

if ! grep -q "^  version \"${version}\"$" "${repo_dir}/${cask_relative}" \
    || ! grep -q "^  sha256 \"${local_sha}\"$" "${repo_dir}/${cask_relative}"; then
    echo "error: Homebrew cask update did not apply cleanly" >&2
    exit 1
fi

if ! git -C "${repo_dir}" diff --quiet -- "${cask_relative}"; then
    git -C "${repo_dir}" add "${cask_relative}"
    git -C "${repo_dir}" commit -q -m "zeus ${version}" -- "${cask_relative}"
fi

# Always push: this is load-bearing for recovery from issue #9, where the
# correct cask commit already existed locally and the remote was still stale.
git -C "${repo_dir}" push --quiet "${remote}" "HEAD:${remote_branch}"
git -C "${repo_dir}" fetch --quiet "${remote}" "${remote_branch}"

remote_cask="$(git -C "${repo_dir}" show "FETCH_HEAD:${cask_relative}")"
remote_version="$(sed -n 's/^  version "\(.*\)"$/\1/p' <<<"${remote_cask}")"
remote_sha="$(sed -n 's/^  sha256 "\([0-9a-f]*\)"$/\1/p' <<<"${remote_cask}")"
if [[ "${remote_version}" != "${version}" || "${remote_sha}" != "${published_sha}" ]]; then
    cat >&2 <<EOF
error: remote Homebrew cask does not match the published DMG
  expected version: ${version}
  remote version:   ${remote_version:-<missing>}
  expected sha256:  ${published_sha}
  remote sha256:    ${remote_sha:-<missing>}
EOF
    exit 1
fi

echo "    Homebrew cask pushed and verified at ${remote}/${remote_branch}"
