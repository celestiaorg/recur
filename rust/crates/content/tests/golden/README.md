Golden outputs recorded from the frontend's own libraries (`frontend/node_modules`), so the
Rust ports are checked against the exact JS behavior:

- `marked.json`: marked 18 (`gfm: true`, `breaks: true`) over the CommonMark 0.31.2 and GFM 0.29
  spec examples and marked's `test/specs/new` cases (v18.0.7).
