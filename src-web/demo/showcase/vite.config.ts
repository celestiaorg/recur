import rawDataset from "./dataset.json";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { editorThemes } from "../../src/lib/themes";

// Build the product views for the website's live preview. The backend is the
// dataset (showcase/transport.ts) plus the shipped editor themes read here.
const root = resolve(import.meta.dirname, "..");
// Avatars ship as local files and inline as data URLs.
const dataset = JSON.parse(
  JSON.stringify(rawDataset).replace(
    /"(avatars\/[\w-]+\.jpg)"/g,
    (_, path) =>
      JSON.stringify(
        "data:image/jpeg;base64," +
          readFileSync(resolve(root, "showcase", path)).toString("base64"),
      ),
  ),
);

/** `[[theme]]` tables from a bundle.toml (flat string/number keys only). */
function bundleThemes(src: string): Record<string, string>[] {
  return src
    .split(/^\[\[theme\]\]\s*$/m)
    .slice(1)
    .map((table) => {
      const theme: Record<string, string> = {};
      for (const m of table.split(/^\[/m)[0].matchAll(/^(\w+)\s*=\s*"([^"]*)"/gm))
        theme[m[1]] = m[2];
      return theme;
    });
}

// The shipped themes the website's visual pickers offer.
const themes: { id: string; name: string; uiTheme: string }[] = [];
const themeFiles: Record<string, string> = {};
for (const bundle of ["Recur", "Themes"]) {
  const dir = resolve(root, "../../bundles", bundle);
  for (const theme of bundleThemes(readFileSync(resolve(dir, "bundle.toml"), "utf8"))) {
    if (!editorThemes.some((t) => t.key === theme.id)) continue;
    themes.push({ id: theme.id, name: theme.name, uiTheme: theme.uiTheme });
    themeFiles[theme.id] = readFileSync(resolve(dir, theme.path), "utf8");
  }
}

export default defineConfig({
  root,
  base: "/demo/",
  plugins: [
    svelte(),
    {
      name: "showcase-fixtures",
      resolveId(id) {
        if (id === "virtual:showcase-fixtures") return "\0" + id;
      },
      load(id) {
        if (id === "\0virtual:showcase-fixtures")
          return `export default ${JSON.stringify({ dataset, themes, themeFiles })}`;
      },
    },
  ],
  build: {
    target: "esnext",
    outDir: resolve(root, "../public/demo"),
    emptyOutDir: true,
    rollupOptions: {
      input: { demo: resolve(root, "showcase/index.html") },
    },
  },
});
