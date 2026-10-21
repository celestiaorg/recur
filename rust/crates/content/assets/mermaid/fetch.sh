#!/usr/bin/env sh
# Copies the mermaid build the web UI ships (frontend/node_modules, pinned by
# frontend/pnpm-lock.yaml) so the native renderer runs the identical library.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src="$here/../../../../../frontend/node_modules/mermaid"
cp "$src/dist/mermaid.min.js" "$here/mermaid.min.js"
cp "$src/LICENSE" "$here/LICENSE"
node -p "require('$src/package.json').version" > "$here/VERSION"
