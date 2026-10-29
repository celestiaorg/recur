Snapshot outputs of the JS libraries the Rust ports follow (marked, DOMPurify), so the ports keep
their behavior. The files are frozen: edit them by hand, or regenerate from Rust, when the
behavior changes on purpose.

- `marked.json`: marked 18 (`gfm: true`, `breaks: true`) over the CommonMark 0.31.2 and GFM 0.29
  spec examples and marked's `test/specs/new` cases (v18.0.7).
- `parse.json`: `parseMarkdown` (`lib/markdown-parse.ts`) over the same corpus plus Recur
  cases, with and without `demoteImages`.
- `purify.json`: DOMPurify 3.4.16 in Playwright WebKit (the app's engine) with the
  `lib/markdown.ts` hook: DOMPurify's own `test/fixtures/expect.mjs` payloads under the default,
  LSP, markdown and Mermaid-SVG configurations, and `sanitizeMarkdown` over every `parse.json`
  output.
