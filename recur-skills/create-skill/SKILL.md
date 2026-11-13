---
name: create-skill
description: >-
  Create Recur Agent Skills. Use when authoring a new skill or asking about
  SKILL.md structure for Recur.
disable-model-invocation: true
---
# Creating Skills in Recur

Skills are markdown folders that teach the agent specialized workflows.

## Locations

| Type | Path |
|------|------|
| Project | `.agents/skills/<name>/` (in the repo; prefer this for shared team skills) |
| User | `~/.agents/skills/<name>/` |
| Shipped Recur | `~/.recur/recur-skills/` (seeded from the app; do not hand-edit: it is overwritten on update) |

Do not expect edits under `~/.recur/recur-skills/` to survive an app upgrade: customize via project or user skill dirs instead.

## SKILL.md

```markdown
---
name: my-skill
description: When to use this skill (shown in list_skills / `/` menu)
disable-model-invocation: true   # optional; slash-only when true
---

# Title
Instructions…
```

`description` is required. Prefer specific trigger language (“Use when…”) over vague help text.

## Activation

- User: `/skill-name` in the composer (host injects skill content).
- Model: `mcp__recur__list_skills` then `mcp__recur__activate_skill` with the name.

Gather purpose, location, triggers, and domain knowledge with `ask_user` before writing files. Create the skill directory and `SKILL.md` directly once requirements are clear.

For Recur Bundles (`bundle.toml`, workflows, commands, grammars), use `create-bundle` instead.
For project run settings (`.recur/project.toml`), use `project-toml` instead.
