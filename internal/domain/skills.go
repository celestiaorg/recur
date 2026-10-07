package domain

// SkillRoots returns relative Agent Skill directory roots (under a worktree or
// $HOME) that Recur scans for SKILL.md folders.
//
// Recur owns discovery and activation (composer `/`, host slash injection,
// mcp__recur__list_skills / activate_skill), so every provider sees the same
// union of common skill locations — not each product's native subset.
// Native provider loaders are suppressed where the product has a switch
// (D-skill-loader). Shipped Recur skills live in ~/.recur/recur-skills
// (seeded from the binary); Cursor's ~/.cursor/skills-cursor is intentionally
// excluded.
func SkillRoots() (workspace, user []string) {
	return []string{
			".cursor/skills",
			".agents/skills",
			".claude/skills",
			".codex/skills",
		}, []string{
			".cursor/skills",
			".agents/skills",
			".claude/skills",
			".codex/skills",
			".recur/recur-skills",
		}
}
