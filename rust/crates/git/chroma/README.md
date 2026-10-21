# Vendored chroma lexers

recur-git highlights diff lines with a port of
[chroma](https://github.com/alecthomas/chroma) v2.27.0, the library the Go
app used, so `codeHtml` stays byte-identical between the two apps. The lexer
definitions here are chroma's own (MIT, see `COPYING`):

- `lexers/` holds chroma's embedded XML lexers, unchanged.
- `generated/` holds the lexers chroma builds in Go code, dumped from chroma
  to the same XML format. Go closures are written as named elements
  (`<func name="…">`, `<bygroupnames>`, `<raku…>`) that
  `src/chroma` implements.
- `remap_*.tsv` are the word tables of the Common Lisp and EmacsLisp
  remapping lexers, and `raku_brackets.tsv` the Raku lexer's bracket pairs.

`regenerate.sh` rebuilds all of this, the Go-derived Unicode and HTML entity
tables, and the parity fixtures in `tests/fixtures/` from a chroma release.
To check the port against chroma's own lexer tests, run
`CHROMA_TESTDATA=<chroma>/lexers/testdata cargo test -p recur-git -- --ignored`.
