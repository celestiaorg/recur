package domain

import (
	"strconv"
	"strings"
)

// RunCommand is one named project run (dev, test, …) from project.toml [[run]].
type RunCommand struct {
	Name string `json:"name"`
	Cmd  string `json:"cmd"`
}

// EffectiveRuns is the named command list the UI and Run button use,
// taken from project.toml [[run]] (p.RunCommands).
func EffectiveRuns(p Project) []RunCommand {
	out := make([]RunCommand, 0, len(p.RunCommands))
	for _, r := range p.RunCommands {
		name := strings.TrimSpace(r.Name)
		cmd := strings.TrimSpace(r.Cmd)
		if cmd == "" {
			continue
		}
		if name == "" {
			name = "run"
		}
		out = append(out, RunCommand{Name: name, Cmd: cmd})
	}
	if len(out) == 0 {
		return nil
	}
	return uniquifyRunNames(out)
}

// DefaultRun is the pinned command when that name still exists, otherwise the first.
func DefaultRun(p Project) RunCommand {
	runs := EffectiveRuns(p)
	if len(runs) == 0 {
		return RunCommand{}
	}
	pin := strings.TrimSpace(p.PinnedRun)
	if pin != "" {
		for _, r := range runs {
			if r.Name == pin {
				return r
			}
		}
	}
	return runs[0]
}

// VerifyDevCmd is the command verify-in-browser boots: a run named "dev" if
// present, otherwise DefaultRun.
func VerifyDevCmd(p Project) string {
	for _, r := range EffectiveRuns(p) {
		if strings.EqualFold(r.Name, "dev") {
			return r.Cmd
		}
	}
	return DefaultRun(p).Cmd
}

// RunsKey is a stable fingerprint of EffectiveRuns for project.toml sync.
func RunsKey(p Project) string {
	var b strings.Builder
	for _, r := range EffectiveRuns(p) {
		b.WriteString(r.Name)
		b.WriteByte('\n')
		b.WriteString(r.Cmd)
		b.WriteByte('\n')
	}
	return b.String()
}

func uniquifyRunNames(runs []RunCommand) []RunCommand {
	used := map[string]int{}
	out := make([]RunCommand, len(runs))
	for i, r := range runs {
		name := r.Name
		n := used[name]
		used[name] = n + 1
		if n > 0 {
			name = name + "-" + strconv.Itoa(n+1)
			for used[name] > 0 {
				n++
				name = r.Name + "-" + strconv.Itoa(n+1)
			}
			used[name] = 1
		}
		out[i] = RunCommand{Name: name, Cmd: r.Cmd}
	}
	return out
}
