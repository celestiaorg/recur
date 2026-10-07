package bundles

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Shipped first-party ids.
const (
	DefaultLoopID    = "recur.loops.pr"
	ConvergeLoopID   = "recur.loops.converge"
	ConvergePRLoopID = "recur.loops.converge-pr"
	DocsLoopID       = "recur.loops.docs"
	SecurityLoopID   = "recur.loops.security"
	// StepLoopID is the engine-synthesized child loop for fan-out (policy.go
	// childLoopDef): never in a bundle, never in the pickers.
	StepLoopID      = "recur.loops.step"
	DefaultPolicyID = "recur.policies.balanced"
	WalkStepCap     = 32
)

// Chat / role modes a [[role]] may declare.
const (
	RoleModeAsk    = "ask"
	RoleModePlan   = "plan"
	RoleModeAgent  = "agent"
	RoleModeReview = "review"
	RoleModeLoop   = "loop"
)

// Closed phase verbs.
const (
	VerbPlan      = "plan"
	VerbImplement = "implement"
	VerbReview    = "review"
	VerbConverge  = "converge"
	VerbVerify    = "verify"
	VerbEnsurePR  = "ensure_pr"
	VerbArtifact  = "artifact"
)

// Reserved phase ids (cannot be author ids).
var reservedPhaseIDs = map[string]bool{
	"fail": true, "escalate": true, "skip": true, "next": true,
}

// Role is one [[role]] entry after parse + prompt-path resolve.
type Role struct {
	ID            string   `toml:"id" json:"id"`
	Name          string   `toml:"name" json:"name"`
	Hint          string   `toml:"hint" json:"hint,omitempty"`
	Code          string   `toml:"code" json:"code"`
	Mode          string   `toml:"mode" json:"mode"`
	Tools         []string `toml:"tools" json:"tools,omitempty"`
	DisabledTools []string `toml:"disabled_tools" json:"disabledTools,omitempty"`
	ModelSlot     string   `toml:"model_slot" json:"modelSlot,omitempty"`
	Prompt        string   `toml:"prompt" json:"prompt,omitempty"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

// Policy is one [[policy]] guard profile.
type Policy struct {
	ID                string            `toml:"id" json:"id"`
	Name              string            `toml:"name" json:"name"`
	Hint              string            `toml:"hint" json:"hint,omitempty"`
	MaxRounds         int               `toml:"max_rounds" json:"maxRounds"`
	OnMaxRounds       string            `toml:"on_max_rounds" json:"onMaxRounds"`
	FindingsThreshold FindingsThreshold `toml:"-" json:"findingsThreshold"`
	PlanApproval      string            `toml:"plan_approval" json:"planApproval"`
	Timeout           string            `toml:"timeout" json:"timeout,omitempty"`
	Retries           int               `toml:"retries" json:"retries"`
	OnFail            string            `toml:"on_fail" json:"onFail"`
	ExtendRounds      int               `toml:"extend_rounds" json:"extendRounds"`
	FindingsRaw       any               `toml:"findings_threshold" json:"-"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

// FindingsThreshold is a per-severity map, or integer 0 (all five at 0).
type FindingsThreshold struct {
	AllZero bool           `json:"allZero,omitempty"`
	Levels  map[string]int `json:"levels,omitempty"`
}

// SeverityLevels is the engine taxonomy, lowest to highest.
var SeverityLevels = []string{"info", "low", "medium", "high", "critical"}

// LoopDef is one [[loop]] after scalar normalize + graph lint.
type LoopDef struct {
	ID      string            `toml:"id" json:"id"`
	Name    string            `toml:"name" json:"name"`
	Hint    string            `toml:"hint" json:"hint,omitempty"`
	Entry   string            `toml:"entry" json:"entry"`
	Reenter []string          `toml:"reenter" json:"reenter,omitempty"`
	Roles   map[string]string `toml:"roles" json:"roles,omitempty"`
	Prompts map[string]string `toml:"prompts" json:"prompts,omitempty"`
	Phases  []LoopPhase       `toml:"phase" json:"phases,omitempty"`
	// Scalar sugar (normalized into Roles/Prompts).
	Conductor string `toml:"conductor" json:"-"`
	Prompt    string `toml:"prompt" json:"-"`

	BundleName string `toml:"-" json:"bundleName"`
	BundleID   string `toml:"-" json:"bundleID"`
	SourceRoot string `toml:"-" json:"sourceRoot"`
	BundleDir  string `toml:"-" json:"bundleDir"`
}

// LoopPhase is one [[loop.phase]].
type LoopPhase struct {
	ID          string             `toml:"id" json:"id"`
	Uses        string             `toml:"uses" json:"uses"`
	Dispatch    string             `toml:"dispatch" json:"dispatch,omitempty"`
	When        string             `toml:"when" json:"when,omitempty"`
	Gate        string             `toml:"gate" json:"gate,omitempty"`
	Next        string             `toml:"next" json:"next,omitempty"`
	OnReject    string             `toml:"on_reject" json:"onReject,omitempty"`
	OnFail      string             `toml:"on_fail" json:"onFail,omitempty"`
	OnMaxRounds string             `toml:"on_max_rounds" json:"onMaxRounds,omitempty"`
	Start       string             `toml:"start" json:"start,omitempty"`
	Timeout     string             `toml:"timeout" json:"timeout,omitempty"`
	MaxRounds   *int               `toml:"max_rounds" json:"maxRounds,omitempty"`
	Retries     *int               `toml:"retries" json:"retries,omitempty"`
	Roles       map[string]string  `toml:"roles" json:"roles,omitempty"`
	Prompts     map[string]string  `toml:"prompts" json:"prompts,omitempty"`
	Role        string             `toml:"role" json:"-"`
	Prompt      string             `toml:"prompt" json:"-"`
	FindingsRaw any                `toml:"findings_threshold" json:"-"`
	Findings    *FindingsThreshold `toml:"-" json:"findingsThreshold,omitempty"`
	Optional    bool               `toml:"optional" json:"-"`
}

type phaseSlotSpec struct {
	roleSlots     []string
	roleModes     map[string][]string // slot → allowed modes
	promptSlots   []string
	requiredRoles []string
}

var verbSlots = map[string]phaseSlotSpec{
	VerbPlan: {
		roleSlots: []string{"planner"}, requiredRoles: []string{"planner"},
		roleModes:   map[string][]string{"planner": {RoleModePlan}},
		promptSlots: []string{"plan"},
	},
	VerbImplement: {
		roleSlots: []string{"implementer"}, requiredRoles: []string{"implementer"},
		roleModes:   map[string][]string{"implementer": {RoleModeAgent}},
		promptSlots: []string{"implement"},
	},
	VerbReview: {
		roleSlots: []string{"reviewer"}, requiredRoles: []string{"reviewer"},
		roleModes:   map[string][]string{"reviewer": {RoleModeReview}},
		promptSlots: []string{"review"},
	},
	VerbConverge: {
		roleSlots:     []string{"implementer", "reviewer"},
		requiredRoles: []string{"implementer", "reviewer"},
		roleModes: map[string][]string{
			"implementer": {RoleModeAgent},
			"reviewer":    {RoleModeReview},
		},
		promptSlots: []string{"implement", "review", "fix", "rereview"},
	},
	VerbVerify: {
		roleSlots: []string{"verifier"}, requiredRoles: []string{"verifier"},
		roleModes:   map[string][]string{"verifier": {RoleModeAsk, RoleModeReview, RoleModeAgent}},
		promptSlots: []string{"verify"},
	},
	VerbEnsurePR: {},
	VerbArtifact: {},
}

func parseRoles(bundleDir string, meta BundleMeta, raw []Role) ([]Role, error) {
	out := make([]Role, 0, len(raw))
	seenCode := map[string]string{}
	for i, r := range raw {
		r.ID = strings.TrimSpace(r.ID)
		if r.ID == "" {
			return nil, fmt.Errorf("role[%d]: id is required", i)
		}
		if !validBundleID(r.ID) {
			return nil, fmt.Errorf("role[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(r.Name) == "" {
			return nil, fmt.Errorf("role %q: name is required", r.ID)
		}
		r.Mode = strings.TrimSpace(r.Mode)
		if r.Mode == "" {
			return nil, fmt.Errorf("role %q: mode is required (ask|plan|agent|review|loop)", r.ID)
		}
		if !validRoleMode(r.Mode) {
			return nil, fmt.Errorf("role %q: unknown mode %q (use ask|plan|agent|review|loop)", r.ID, r.Mode)
		}
		r.Code = strings.ToUpper(strings.TrimSpace(r.Code))
		if len(r.Code) != 2 {
			return nil, fmt.Errorf("role %q: code must be 2 letters", r.ID)
		}
		for _, ch := range r.Code {
			if ch < 'A' || ch > 'Z' {
				return nil, fmt.Errorf("role %q: code must be 2 letters", r.ID)
			}
		}
		if other, ok := seenCode[r.Code]; ok {
			return nil, fmt.Errorf("role %q: code %q collides with %q", r.ID, r.Code, other)
		}
		seenCode[r.Code] = r.ID
		if strings.TrimSpace(r.ModelSlot) == "" {
			r.ModelSlot = r.Mode
		}
		if err := resolvePromptField(bundleDir, r.ID, "role", &r.Prompt); err != nil {
			return nil, err
		}
		r.BundleName = meta.Name
		r.BundleID = meta.ID
		r.BundleDir = bundleDir
		out = append(out, r)
	}
	return out, nil
}

func parsePolicies(bundleDir string, meta BundleMeta, raw []Policy) ([]Policy, error) {
	out := make([]Policy, 0, len(raw))
	for i, p := range raw {
		p.ID = strings.TrimSpace(p.ID)
		if p.ID == "" {
			return nil, fmt.Errorf("policy[%d]: id is required", i)
		}
		if !validBundleID(p.ID) {
			return nil, fmt.Errorf("policy[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(p.Name) == "" {
			return nil, fmt.Errorf("policy %q: name is required", p.ID)
		}
		if p.MaxRounds == 0 {
			p.MaxRounds = 4
		}
		if p.MaxRounds < 1 {
			return nil, fmt.Errorf("policy %q: max_rounds must be >= 1", p.ID)
		}
		if p.OnMaxRounds == "" {
			p.OnMaxRounds = "escalate"
		}
		if !validOnMaxRounds(p.OnMaxRounds) {
			return nil, fmt.Errorf("policy %q: on_max_rounds must be fail|escalate|proceed", p.ID)
		}
		if p.PlanApproval == "" {
			p.PlanApproval = "if_structured"
		}
		if !validPlanApproval(p.PlanApproval) {
			return nil, fmt.Errorf("policy %q: plan_approval must be always|if_structured|never", p.ID)
		}
		if p.OnFail == "" {
			p.OnFail = "fail"
		}
		if !validOnFail(p.OnFail) {
			return nil, fmt.Errorf("policy %q: on_fail must be fail|escalate|skip", p.ID)
		}
		if p.ExtendRounds < 0 {
			return nil, fmt.Errorf("policy %q: extend_rounds must be >= 0", p.ID)
		}
		th, err := parseFindingsThreshold(p.FindingsRaw, false)
		if err != nil {
			return nil, fmt.Errorf("policy %q: %w", p.ID, err)
		}
		p.FindingsThreshold = th
		p.FindingsRaw = nil
		p.BundleName = meta.Name
		p.BundleID = meta.ID
		p.BundleDir = bundleDir
		out = append(out, p)
	}
	return out, nil
}

func parseLoops(bundleDir string, meta BundleMeta, raw []LoopDef) ([]LoopDef, error) {
	out := make([]LoopDef, 0, len(raw))
	for i, lp := range raw {
		lp.ID = strings.TrimSpace(lp.ID)
		if lp.ID == "" {
			return nil, fmt.Errorf("loop[%d]: id is required", i)
		}
		if !validBundleID(lp.ID) {
			return nil, fmt.Errorf("loop[%d]: id is not a safe id", i)
		}
		if strings.TrimSpace(lp.Name) == "" {
			return nil, fmt.Errorf("loop %q: name is required", lp.ID)
		}
		if err := normalizeLoopSlots(&lp); err != nil {
			return nil, fmt.Errorf("loop %q: %w", lp.ID, err)
		}
		for slot, val := range lp.Prompts {
			v := val
			if err := resolvePromptField(bundleDir, lp.ID, "prompts."+slot, &v); err != nil {
				return nil, fmt.Errorf("loop %q: %w", lp.ID, err)
			}
			lp.Prompts[slot] = v
		}
		if strings.TrimSpace(lp.Entry) == "" {
			return nil, fmt.Errorf("loop %q: entry is required", lp.ID)
		}
		seenPhase := map[string]bool{}
		for j := range lp.Phases {
			if err := normalizePhase(&lp.Phases[j], bundleDir); err != nil {
				return nil, fmt.Errorf("loop %q phase[%d]: %w", lp.ID, j, err)
			}
			id := lp.Phases[j].ID
			if seenPhase[id] {
				return nil, fmt.Errorf("loop %q: duplicate phase id %q", lp.ID, id)
			}
			seenPhase[id] = true
		}
		if err := lintLoopGraph(lp); err != nil {
			return nil, fmt.Errorf("loop %q: %w", lp.ID, err)
		}
		lp.BundleName = meta.Name
		lp.BundleID = meta.ID
		lp.BundleDir = bundleDir
		out = append(out, lp)
	}
	return out, nil
}

func normalizeLoopSlots(lp *LoopDef) error {
	if lp.Roles == nil {
		lp.Roles = map[string]string{}
	}
	if lp.Prompts == nil {
		lp.Prompts = map[string]string{}
	}
	if lp.Conductor != "" {
		if lp.Roles["conductor"] != "" && lp.Roles["conductor"] != lp.Conductor {
			return fmt.Errorf("conductor and roles.conductor both set")
		}
		lp.Roles["conductor"] = strings.TrimSpace(lp.Conductor)
		lp.Conductor = ""
	}
	if lp.Prompt != "" {
		if lp.Prompts["mission"] != "" && lp.Prompts["mission"] != lp.Prompt {
			return fmt.Errorf("prompt and prompts.mission both set")
		}
		lp.Prompts["mission"] = lp.Prompt
		lp.Prompt = ""
	}
	if strings.TrimSpace(lp.Roles["conductor"]) == "" {
		return fmt.Errorf("roles.conductor is required")
	}
	return nil
}

func normalizePhase(ph *LoopPhase, bundleDir string) error {
	ph.ID = strings.TrimSpace(ph.ID)
	if ph.ID == "" {
		return fmt.Errorf("id is required")
	}
	if reservedPhaseIDs[ph.ID] {
		return fmt.Errorf("phase id %q is reserved", ph.ID)
	}
	ph.Uses = strings.TrimSpace(ph.Uses)
	spec, ok := verbSlots[ph.Uses]
	if !ok {
		return fmt.Errorf("unknown uses %q", ph.Uses)
	}
	if ph.Optional {
		return fmt.Errorf("optional is reserved; omit it or use dispatch = \"on_demand\"")
	}
	if ph.Dispatch == "" {
		ph.Dispatch = "graph"
	}
	if ph.Dispatch != "graph" && ph.Dispatch != "on_demand" {
		return fmt.Errorf("dispatch must be graph|on_demand")
	}
	if ph.Roles == nil {
		ph.Roles = map[string]string{}
	}
	if ph.Prompts == nil {
		ph.Prompts = map[string]string{}
	}
	roleArity := len(spec.roleSlots)
	if ph.Role != "" {
		if roleArity != 1 {
			return fmt.Errorf("scalar role = is only legal when uses %q has exactly one role slot", ph.Uses)
		}
		if ph.Roles[spec.roleSlots[0]] != "" && ph.Roles[spec.roleSlots[0]] != ph.Role {
			return fmt.Errorf("role and roles.%s both set", spec.roleSlots[0])
		}
		ph.Roles[spec.roleSlots[0]] = strings.TrimSpace(ph.Role)
		ph.Role = ""
	}
	if ph.Prompt != "" {
		if len(spec.promptSlots) != 1 {
			return fmt.Errorf("scalar prompt = is only legal when uses %q has exactly one prompt slot (use prompts)", ph.Uses)
		}
		slot := spec.promptSlots[0]
		if ph.Prompts[slot] != "" && ph.Prompts[slot] != ph.Prompt {
			return fmt.Errorf("prompt and prompts.%s both set", slot)
		}
		ph.Prompts[slot] = ph.Prompt
		ph.Prompt = ""
	}
	for k := range ph.Roles {
		if k == "" {
			return fmt.Errorf("empty role slot for uses %q", ph.Uses)
		}
		if !containsStr(spec.roleSlots, k) {
			return fmt.Errorf("unknown role slot %q for uses %q", k, ph.Uses)
		}
	}
	for k := range ph.Prompts {
		if !containsStr(spec.promptSlots, k) {
			return fmt.Errorf("unknown prompt slot %q for uses %q", k, ph.Uses)
		}
	}
	for _, slot := range spec.requiredRoles {
		if strings.TrimSpace(ph.Roles[slot]) == "" {
			return fmt.Errorf("roles.%s is required for uses %q", slot, ph.Uses)
		}
	}
	if ph.Start != "" {
		if ph.Uses != VerbConverge {
			return fmt.Errorf("start is only legal on converge")
		}
		if ph.Start != "implement" && ph.Start != "review" {
			return fmt.Errorf("start must be implement|review")
		}
	}
	if ph.Gate != "" && ph.Gate != "plan_approval" {
		return fmt.Errorf("unknown gate %q", ph.Gate)
	}
	if ph.OnFail != "" && !validOnFail(ph.OnFail) {
		return fmt.Errorf("on_fail must be fail|escalate|skip")
	}
	if ph.OnMaxRounds != "" && !validOnMaxRounds(ph.OnMaxRounds) {
		return fmt.Errorf("on_max_rounds must be fail|escalate|proceed")
	}
	if ph.MaxRounds != nil && *ph.MaxRounds < 1 {
		return fmt.Errorf("max_rounds must be >= 1")
	}
	if ph.FindingsRaw != nil {
		th, err := parseFindingsThreshold(ph.FindingsRaw, true)
		if err != nil {
			return err
		}
		ph.Findings = &th
		ph.FindingsRaw = nil
	}
	for slot, val := range ph.Prompts {
		v := val
		if err := resolvePromptField(bundleDir, ph.ID, "prompts."+slot, &v); err != nil {
			return err
		}
		ph.Prompts[slot] = v
	}
	return nil
}

func lintLoopGraph(lp LoopDef) error {
	byID := map[string]LoopPhase{}
	for _, ph := range lp.Phases {
		byID[ph.ID] = ph
	}
	if _, ok := byID[lp.Entry]; !ok {
		return fmt.Errorf("entry %q is not a phase", lp.Entry)
	}
	if byID[lp.Entry].Dispatch == "on_demand" {
		return fmt.Errorf("entry %q is on-demand", lp.Entry)
	}
	for _, id := range lp.Reenter {
		ph, ok := byID[id]
		if !ok {
			return fmt.Errorf("reenter %q is not a phase", id)
		}
		if ph.Dispatch == "on_demand" {
			return fmt.Errorf("reenter cannot list on-demand id %q", id)
		}
	}
	// inbound next onto on-demand is illegal
	inbound := map[string][]string{}
	for _, ph := range lp.Phases {
		for _, edge := range []string{ph.Next, ph.OnReject} {
			if edge == "" {
				continue
			}
			if reservedPhaseIDs[edge] {
				return fmt.Errorf("phase %q: %q is reserved (not a phase id)", ph.ID, edge)
			}
			if _, ok := byID[edge]; !ok {
				return fmt.Errorf("phase %q names missing id %q", ph.ID, edge)
			}
			inbound[edge] = append(inbound[edge], ph.ID)
		}
	}
	for id, ph := range byID {
		if ph.Dispatch == "on_demand" && len(inbound[id]) > 0 {
			return fmt.Errorf("on-demand phase %q has inbound next", id)
		}
	}
	// success-next DAG
	adj := map[string][]string{}
	for _, ph := range lp.Phases {
		if ph.Dispatch == "on_demand" {
			continue
		}
		if ph.Next != "" {
			adj[ph.ID] = append(adj[ph.ID], ph.Next)
		}
	}
	if cycle := graphCycle(adj); cycle != "" {
		return fmt.Errorf("next graph contains a cycle (%s)", cycle)
	}
	return nil
}

func graphCycle(adj map[string][]string) string {
	const (
		white = 0
		gray  = 1
		black = 2
	)
	color := map[string]int{}
	var walk func(string) string
	walk = func(n string) string {
		color[n] = gray
		for _, m := range adj[n] {
			switch color[m] {
			case gray:
				return n + " → " + m
			case white:
				if c := walk(m); c != "" {
					return c
				}
			}
		}
		color[n] = black
		return ""
	}
	for n := range adj {
		if color[n] == white {
			if c := walk(n); c != "" {
				return c
			}
		}
	}
	return ""
}

func parseFindingsThreshold(raw any, phase bool) (FindingsThreshold, error) {
	if raw == nil {
		return FindingsThreshold{}, nil
	}
	switch v := raw.(type) {
	case int64:
		if v != 0 {
			if phase {
				return FindingsThreshold{}, fmt.Errorf("phase findings_threshold integer must be 0 (got %d)", v)
			}
			return FindingsThreshold{}, fmt.Errorf("integer findings_threshold must be 0")
		}
		return FindingsThreshold{AllZero: true}, nil
	case int:
		if v != 0 {
			if phase {
				return FindingsThreshold{}, fmt.Errorf("phase findings_threshold integer must be 0 (got %d)", v)
			}
			return FindingsThreshold{}, fmt.Errorf("integer findings_threshold must be 0")
		}
		return FindingsThreshold{AllZero: true}, nil
	case float64:
		if v != 0 {
			return FindingsThreshold{}, fmt.Errorf("integer findings_threshold must be 0")
		}
		return FindingsThreshold{AllZero: true}, nil
	case map[string]any:
		levels := map[string]int{}
		for k, val := range v {
			k = strings.ToLower(strings.TrimSpace(k))
			if !validSeverity(k) {
				return FindingsThreshold{}, fmt.Errorf("unknown severity %q", k)
			}
			n, err := anyToInt(val)
			if err != nil {
				return FindingsThreshold{}, fmt.Errorf("findings_threshold.%s: %w", k, err)
			}
			if n < 0 {
				return FindingsThreshold{}, fmt.Errorf("findings_threshold.%s must be >= 0", k)
			}
			levels[k] = n
		}
		return FindingsThreshold{Levels: levels}, nil
	default:
		return FindingsThreshold{}, fmt.Errorf("findings_threshold must be a map or 0")
	}
}

func anyToInt(v any) (int, error) {
	switch n := v.(type) {
	case int64:
		return int(n), nil
	case int:
		return n, nil
	case float64:
		if n != float64(int(n)) {
			return 0, fmt.Errorf("not an integer")
		}
		return int(n), nil
	case string:
		return strconv.Atoi(n)
	default:
		return 0, fmt.Errorf("not an integer")
	}
}

// LooksLikePromptPath reports whether s should be resolved as a bundle-relative
// file.
func LooksLikePromptPath(s string) bool {
	s = strings.TrimSpace(s)
	if s == "" {
		return false
	}
	if s[0] == '@' || s[0] == '/' {
		return false
	}
	if strings.ContainsAny(s, " \t\n") {
		return false
	}
	return strings.Contains(s, "/") || strings.HasSuffix(strings.ToLower(s), ".md")
}

func resolvePromptField(bundleDir, owner, field string, slot *string) error {
	s := strings.TrimSpace(*slot)
	if s == "" || !LooksLikePromptPath(s) {
		*slot = s
		return nil
	}
	if filepath.IsAbs(s) {
		return fmt.Errorf("%s %s: path %q escapes the bundle", owner, field, s)
	}
	for _, part := range strings.Split(filepath.ToSlash(s), "/") {
		if part == ".." {
			return fmt.Errorf("%s %s: path %q escapes the bundle", owner, field, s)
		}
	}
	p := filepath.Join(bundleDir, filepath.FromSlash(s))
	rel, err := filepath.Rel(bundleDir, p)
	if err != nil || strings.HasPrefix(rel, "..") {
		return fmt.Errorf("%s %s: path %q escapes the bundle", owner, field, s)
	}
	body, err := os.ReadFile(p)
	if err != nil {
		return fmt.Errorf("%s %s: missing path %q", owner, field, s)
	}
	*slot = string(body)
	return nil
}

func validRoleMode(s string) bool {
	switch s {
	case RoleModeAsk, RoleModePlan, RoleModeAgent, RoleModeReview, RoleModeLoop:
		return true
	default:
		return false
	}
}

func validOnMaxRounds(s string) bool {
	switch s {
	case "fail", "escalate", "proceed":
		return true
	default:
		return false
	}
}

func validOnFail(s string) bool {
	switch s {
	case "fail", "escalate", "skip":
		return true
	default:
		return false
	}
}

func validPlanApproval(s string) bool {
	switch s {
	case "always", "if_structured", "never":
		return true
	default:
		return false
	}
}

func validSeverity(s string) bool {
	for _, lv := range SeverityLevels {
		if lv == s {
			return true
		}
	}
	return false
}

func containsStr(xs []string, s string) bool {
	for _, x := range xs {
		if x == s {
			return true
		}
	}
	return false
}

// PhaseByID returns the phase or false.
func (lp LoopDef) PhaseByID(id string) (LoopPhase, bool) {
	for _, ph := range lp.Phases {
		if ph.ID == id {
			return ph, true
		}
	}
	return LoopPhase{}, false
}

// GraphPhaseIDs are non-on-demand phases (default reenter list).
func (lp LoopDef) GraphPhaseIDs() []string {
	var out []string
	for _, ph := range lp.Phases {
		if ph.Dispatch != "on_demand" && ph.Uses != VerbEnsurePR && ph.Uses != VerbArtifact {
			out = append(out, ph.ID)
		}
	}
	return out
}

// ReenterIDs is the author list, or every graph non-terminal when omitted.
func (lp LoopDef) ReenterIDs() []string {
	if len(lp.Reenter) > 0 {
		return append([]string(nil), lp.Reenter...)
	}
	return lp.GraphPhaseIDs()
}

// AllowedRoleModes returns the modes a slot may fill for uses.
func AllowedRoleModes(uses, slot string) []string {
	spec, ok := verbSlots[uses]
	if !ok {
		return nil
	}
	return spec.roleModes[slot]
}

// FindingsBlock reports whether open findings fail the threshold.
// Omitted map levels do not participate. AllZero = every severity at 0.
func FindingsBlock(th FindingsThreshold, counts map[string]int) bool {
	if th.AllZero {
		for _, lv := range SeverityLevels {
			if counts[lv] > 0 {
				return true
			}
		}
		return false
	}
	for lv, cap := range th.Levels {
		if counts[lv] > cap {
			return true
		}
	}
	return false
}

// SameOrStricter reports whether phase is same-or-stricter than policy.
func SameOrStricter(phase, policy FindingsThreshold) error {
	if phase.AllZero {
		return nil
	}
	if policy.AllZero {
		for _, n := range phase.Levels {
			if n > 0 {
				return fmt.Errorf("phase findings_threshold is looser than policy (all-zero)")
			}
		}
		return nil
	}
	for lv, pCap := range policy.Levels {
		phCap, named := phase.Levels[lv]
		if !named {
			return fmt.Errorf("phase drops policy level %s", lv)
		}
		if phCap > pCap {
			return fmt.Errorf("phase raises %s cap from %d to %d", lv, pCap, phCap)
		}
	}
	return nil
}

// FindRole returns the catalog role by id.
func FindRole(roles []Role, id string) (Role, bool) {
	for _, r := range roles {
		if r.ID == id {
			return r, true
		}
	}
	return Role{}, false
}

// FindLoop returns the catalog loop by id.
func FindLoop(loops []LoopDef, id string) (LoopDef, bool) {
	for _, l := range loops {
		if l.ID == id {
			return l, true
		}
	}
	return LoopDef{}, false
}

// FindPolicy returns the catalog policy by id.
func FindPolicy(policies []Policy, id string) (Policy, bool) {
	for _, p := range policies {
		if p.ID == id {
			return p, true
		}
	}
	return Policy{}, false
}

// PolicyFromAutonomy maps a stored autonomy level onto a shipped policy id.
func PolicyFromAutonomy(a string) string {
	switch strings.ToLower(strings.TrimSpace(a)) {
	case "cautious":
		return "recur.policies.cautious"
	case "aggressive":
		return "recur.policies.aggressive"
	default:
		return DefaultPolicyID
	}
}
