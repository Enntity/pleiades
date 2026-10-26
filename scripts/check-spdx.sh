#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

fail=0
while IFS= read -r -d '' file; do
    case "$file" in
        LICENSE|Cargo.lock) continue ;;
    esac
    case "$file" in
        *.rs|*.sh|*.toml|*.yml|*.yaml|*.md) ;;
        *) continue ;;
    esac

    line1="$(sed -n '1p' "$file")"
    if [[ "$line1" == *"SPDX-License-Identifier: AGPL-3.0-only"* ]]; then
        continue
    fi
    if [[ "$line1" == "#!"* ]] && \
       [[ "$(sed -n '2p' "$file")" == *"SPDX-License-Identifier: AGPL-3.0-only"* ]]; then
        continue
    fi

    echo "$file"
    fail=1
done < <( { git ls-files -z; git ls-files --others --exclude-standard -z; } )

exit "$fail"
