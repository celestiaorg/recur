# recur-content

Rich content shared by the chat, PR, artifact and file views.

## Usage

Render settled markdown with `MarkdownView`, and live replies with `StreamingMarkdown`:

```rust
let view = MarkdownView::new(text, MarkdownStyle::doc());
```

### Notes

- Markdown is sanitized before it renders.
- Mermaid fences become diagrams.

> Previews follow the file as it changes.
