The review flow:

```mermaid
sequenceDiagram
  participant U as User
  participant A as Agent
  U->>A: Ask for a review
  A-->>U: Findings
  Note over U,A: Rendered natively
```
