#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
revision=$(git rev-parse :linux)

git submodule init linux
git init linux
git -C linux config remote.origin.url https://github.com/torvalds/linux
git -C linux sparse-checkout set --cone \
    drivers/net/ethernet/intel/e1000 include arch/x86/include
git -C linux fetch --depth 1 --no-tags --filter=blob:none origin "$revision"
git -C linux checkout --detach "$revision"
git submodule absorbgitdirs linux
