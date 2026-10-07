package codex

import (
	"strings"
	"testing"

	toml "github.com/pelletier/go-toml/v2"
)

func TestConfigOverridesMCPURL(t *testing.T) {
	ov := ConfigOverrides([]MCPServer{{
		Name:     "recur",
		URL:      "http://127.0.0.1:9/tok/mcp",
		Required: true,
	}})
	joined := strings.Join(ov, "\n")
	for _, want := range []string{
		`mcp_servers.recur.url="http://127.0.0.1:9/tok/mcp"`,
		`mcp_servers.recur.required=true`,
		`mcp_servers.recur.default_tools_approval_mode="approve"`,
		`approval_policy="never"`,
		`model_reasoning_summary="auto"`,
		`model_supports_reasoning_summaries=true`,
		`skills.bundled.enabled=false`,
		`skills.include_instructions=false`,
		`features.skip_host_skill_discovery=true`,
		`features.remote_plugin=false`,
		`features.multi_agent=false`,
		`features.multi_agent_v2=false`,
	} {
		if !strings.Contains(joined, want) {
			t.Fatalf("overrides missing %q\n%s", want, joined)
		}
	}
	if strings.Contains(joined, "network_access") {
		t.Fatalf("default overrides must not lift sandbox network:\n%s", joined)
	}
	args := ConfigArgs(ov)
	if len(args) != len(ov)*2 {
		t.Fatalf("ConfigArgs len=%d want %d", len(args), len(ov)*2)
	}
	for i := 0; i < len(args); i += 2 {
		if args[i] != "-c" {
			t.Fatalf("argv[%d]=%q want -c", i, args[i])
		}
	}
}

func TestConfigOverridesPlaywrightStdio(t *testing.T) {
	ov := ConfigOverrides([]MCPServer{{
		Name:    "playwright",
		Command: "npx",
		Args:    []string{"-y", "@playwright/mcp@latest"},
	}})
	joined := strings.Join(ov, "\n")
	if !strings.Contains(joined, `mcp_servers.playwright.command="npx"`) {
		t.Fatalf("missing command:\n%s", joined)
	}
	if !strings.Contains(joined, `mcp_servers.playwright.args=["-y","@playwright/mcp@latest"]`) {
		t.Fatalf("missing args:\n%s", joined)
	}
}

func TestConfigOverridesHTTPHeaders(t *testing.T) {
	ov := ConfigOverrides([]MCPServer{{
		Name: "linear",
		URL:  "https://mcp.linear.app/mcp",
		HTTPHeaders: map[string]string{
			"Authorization": "Bearer lin_api_x",
		},
	}})
	joined := strings.Join(ov, "\n")
	if !strings.Contains(joined, `mcp_servers.linear.url="https://mcp.linear.app/mcp"`) {
		t.Fatalf("missing url:\n%s", joined)
	}
	if !strings.Contains(joined, `mcp_servers.linear.http_headers.Authorization="Bearer lin_api_x"`) {
		t.Fatalf("missing header:\n%s", joined)
	}
	if strings.Contains(joined, `mcp_servers.linear.required=true`) {
		t.Fatal("user MCP must not be required")
	}
}

func TestConfigOverridesStdioEnv(t *testing.T) {
	ov := ConfigOverrides([]MCPServer{{
		Name:    "fs",
		Command: "npx",
		Args:    []string{"-y", "mcp-fs"},
		Env:     map[string]string{"FOO": "bar"},
	}})
	joined := strings.Join(ov, "\n")
	if !strings.Contains(joined, `mcp_servers.fs.command="npx"`) {
		t.Fatalf("missing command:\n%s", joined)
	}
	if !strings.Contains(joined, `mcp_servers.fs.env.FOO="bar"`) {
		t.Fatalf("missing env:\n%s", joined)
	}
	if strings.Contains(joined, `mcp_servers.fs.required=true`) {
		t.Fatal("user MCP must not be required")
	}
}

func TestConfigOverridesStdioEnvMultiline(t *testing.T) {
	pem := "-----BEGIN KEY-----\nline2\n-----END KEY-----"
	ov := ConfigOverrides([]MCPServer{{
		Name:    "fs",
		Command: "npx",
		Env:     map[string]string{"PRIVATE_KEY": pem},
	}})
	var raw string
	for _, o := range ov {
		if rest, ok := strings.CutPrefix(o, `mcp_servers.fs.env.PRIVATE_KEY=`); ok {
			raw = rest
			break
		}
	}
	if raw == "" {
		t.Fatalf("ConfigOverrides missing PRIVATE_KEY:\n%s", strings.Join(ov, "\n"))
	}
	if strings.Contains(raw, "\n") {
		t.Fatalf("ConfigOverrides PRIVATE_KEY contains a literal newline: %q", raw)
	}
	var got struct {
		V string `toml:"v"`
	}
	doc := "v = " + raw
	if err := toml.Unmarshal([]byte(doc), &got); err != nil {
		t.Fatalf("toml.Unmarshal(%s) = %v", doc, err)
	}
	if got.V != pem {
		t.Fatalf("toml round-trip = %q, want %q", got.V, pem)
	}
}
