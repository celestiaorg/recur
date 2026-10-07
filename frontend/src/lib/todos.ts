// TodoWrite / Cursor updateTodos checklist helpers. Named "todo", not "plan":
// this is a flat per-agent checklist, distinct from the workspace Plan artifact.

export type Todo = { content: string; status: string }

export function parseTodos(input: string | undefined | null): Todo[] {
  if (!input) return []
  let raw: unknown
  try {
    const v = JSON.parse(input)
    raw = v && typeof v === 'object' ? (v as { todos?: unknown }).todos : undefined
  } catch {
    return []
  }
  if (!Array.isArray(raw)) return []
  return raw
    .filter((t) => t && typeof t === 'object')
    .map((t: { content?: unknown; status?: unknown }) => ({
      content: typeof t.content === 'string' ? t.content : '',
      status: typeof t.status === 'string' ? t.status : 'pending',
    }))
    .filter((t) => t.content.trim())
}

export function todoCounts(todos: Todo[]): { done: number; total: number } {
  return { done: todos.filter((t) => t.status === 'completed').length, total: todos.length }
}

/** Latest TodoWrite payload in a transcript (agents replace the whole list). */
export function latestTodosFromMessages(msgs: { tool?: string; input?: string }[]): Todo[] {
  for (let i = msgs.length - 1; i >= 0; i--) {
    const m = msgs[i]
    if (m.tool !== 'TodoWrite') continue
    const todos = parseTodos(m.input)
    if (todos.length) return todos
  }
  return []
}
