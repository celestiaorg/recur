/// <reference types="vitest/config" />
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin, type ViteDevServer } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import wails from "@wailsio/runtime/plugins/vite";

// Vite `emptyOutDir` (default true) wipes frontend/dist on every build,
// including the tracked .gitkeep that keeps //go:embed all:frontend/dist
// valid on a fresh clone. emitFile writes it into the resolved outDir.
function keepDistGitkeep() {
  return {
    name: "keep-dist-gitkeep",
    apply: "build" as const,
    generateBundle() {
      this.emitFile({ type: "asset" as const, fileName: ".gitkeep", source: "" });
    },
  };
}

const recurDebug =
  process.env.RECUR_DEBUG === "1" || process.env.RECUR_DEBUG === "true";

const PDFJS_ASSET_DIRS = ["cmaps", "standard_fonts", "wasm"] as const;

function pdfjsDistRoot(): string {
  return path.dirname(fileURLToPath(import.meta.resolve("pdfjs-dist/package.json")));
}

function pdfjsMime(file: string): string {
  if (file.endsWith(".wasm")) return "application/wasm";
  if (file.endsWith(".js")) return "text/javascript";
  if (file.endsWith(".ttf")) return "font/ttf";
  return "application/octet-stream";
}

function pdfjsAssets(): Plugin {
  return {
    name: "pdfjs-assets",
    configureServer(server: ViteDevServer) {
      const root = pdfjsDistRoot();
      server.middlewares.use((req, res, next) => {
        const raw = req.url?.split("?")[0] ?? "";
        if (!raw.startsWith("/pdfjs/")) return next();
        const rel = decodeURIComponent(raw.slice("/pdfjs/".length));
        const file = path.resolve(root, rel);
        const ok = PDFJS_ASSET_DIRS.some((dir) => {
          const dirPath = path.resolve(root, dir);
          return file === dirPath || file.startsWith(dirPath + path.sep);
        });
        if (!ok || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
          res.statusCode = 404;
          res.end();
          return;
        }
        res.setHeader("Content-Type", pdfjsMime(file));
        fs.createReadStream(file).pipe(res);
      });
    },
    generateBundle() {
      const root = pdfjsDistRoot();
      for (const dir of PDFJS_ASSET_DIRS) {
        const abs = path.join(root, dir);
        for (const name of fs.readdirSync(abs)) {
          const file = path.join(abs, name);
          if (!fs.statSync(file).isFile()) continue;
          this.emitFile({
            type: "asset",
            fileName: `pdfjs/${dir}/${name}`,
            source: fs.readFileSync(file),
          });
        }
      }
    },
  };
}

// https://vitejs.dev/config/
export default defineConfig({
  // Component tests mount the client runtime in happy-dom.
  resolve: process.env.VITEST ? { conditions: ["browser"] } : undefined,
  server: {
    host: "127.0.0.1",
    port: Number(process.env.WAILS_VITE_PORT) || 9245,
    strictPort: true,
  },
  plugins: [svelte(), wails("./bindings"), keepDistGitkeep(), pdfjsAssets()],
  // Packaged `RECUR_DEBUG=1` builds keep the web inspector; leave JS/CSS
  // readable and emit sourcemaps so stack traces map back to Svelte/TS.
  build: recurDebug
    ? { minify: false, sourcemap: true }
    : {},
  test: {
    // Unit tests for pure lib helpers. Playwright e2e stays under e2e/.
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
