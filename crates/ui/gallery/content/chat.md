# Release notes

A paragraph with **bold**, *emphasis*, `inline code`, ~~struck~~ and a [link to the PR](https://github.com/org/repo/pull/12). This sentence keeps going so it wraps across more than one line in the chat column, the way long agent replies read.

## Changes

- Parse markdown off the UI thread
- Highlight fences with `codehl`
  - nested item one
  - nested item two
- [x] Sanitize with ammonia

1. First step
2. Second step

> A blockquote with a note
> that spans two lines.

---

| Crate | Lines | Status |
|:------|------:|--------|
| `recur-content` | 4200 | **done** |
| `recur-ui` | 15600 | shared |

```go
func main() {
	fmt.Println("hello") // greet
}
```

```ts
const answer: number = 42
export function double(x: number) { return x * 2 }
```

```bash
$ cargo test -p recur-content --lib
```

![chart](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAPAAAAB4CAIAAABD1OhwAAABQklEQVR42u3dsQ2DMBBAUTtiELeRmIDKaREzZAyqlFSMwQwWLV1WYJ0UWSChQFH8Xg3N6etkuXFMbQ7wLy5GgKBB0CBoEDSCBkGDoEHQIGgEDYIGQYOgQdDUpPn2h+djN7W3brp+/vEwbyZ2TBlvNjSOHCBoEDQIGgSNoEHQIGgQNAgaQYOgQdAgaBA0ggZBg6BB0CBoBA2CBkGDoEHQCBoEDYIGQYOgETQIGgQNgkbQIGgQNAgaBE3w8CbhF16PxIYGQSNoEDQIGgRNcG3HCe6lN4RjlmG1oXHkAEGDoEHQIGgEDYIGQYOgQdAIGgQNggZBg6ARNAgaBA2CBkEjaBA0CBoEDYJG0CBoEDQIGgSNoEHQIGgQNIIGQYOgQdAgaGoUU5tNARsaBA2CBkEjaBA0CBoEDYJG0CBoEDQIGgRNNV53PAx4pGNXeAAAAABJRU5ErkJggg==)
