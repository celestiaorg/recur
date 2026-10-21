#!/usr/bin/env sh
# Copies the pdf.js build the web UI ships (frontend/node_modules, pinned by
# frontend/pnpm-lock.yaml) and the asset dirs `pdfjsAssetDirs` points it at, so the native
# preview runs the identical library.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
src="$here/../../../../../frontend/node_modules/pdfjs-dist"
cp "$src/build/pdf.min.mjs" "$src/build/pdf.worker.min.mjs" "$src/LICENSE" "$here/"
for d in cmaps standard_fonts wasm; do
  rm -rf "${here:?}/$d"
  cp -R "$src/$d" "$here/$d"
done
# The scripting sandbox's engine; the preview never enables PDF scripting.
rm -f "$here/wasm/quickjs-eval.js" "$here/wasm/quickjs-eval.wasm"
node -p "require('$src/package.json').version" > "$here/VERSION"
