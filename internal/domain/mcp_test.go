package domain

import (
	"reflect"
	"testing"
)

func TestValidMCPID(t *testing.T) {
	if ValidMCPID("recur") || ValidMCPID("playwright") || ValidMCPID("") || ValidMCPID("Linear") {
		t.Fatal("reserved / empty / uppercase must be invalid")
	}
	if !ValidMCPID("linear") || !ValidMCPID("acme-jira") {
		t.Fatal("linear and custom slugs must be valid")
	}
}

func TestMCPCatalogLinearDefaultOff(t *testing.T) {
	cat := MCPCatalog(nil)
	if len(cat) != 1 || cat[0].ID != MCPIDLinear || cat[0].Enabled {
		t.Fatalf("catalog=%+v, want Linear disabled", cat)
	}
	if off := MCPGlobalDisabled(nil); !reflect.DeepEqual(off, []string{MCPIDLinear}) {
		t.Fatalf("global disabled=%v", off)
	}
	if EffectiveSkillEnabled(MCPIDLinear, MCPGlobalDisabled(nil), nil, nil) {
		t.Fatal("linear should be off until enabled")
	}
}

func TestMCPCatalogLinearEnabled(t *testing.T) {
	stored := []MCPServer{{ID: "linear", Enabled: true}}
	cat := MCPCatalog(stored)
	if len(cat) != 1 || !cat[0].Enabled || cat[0].Preset != MCPPresetLinear {
		t.Fatalf("catalog=%+v", cat)
	}
	off := MCPGlobalDisabled(stored)
	if EffectiveSkillEnabled(MCPIDLinear, off, nil, nil) != true {
		t.Fatal("enabled linear should be on")
	}
	if EffectiveSkillEnabled(MCPIDLinear, off, []string{MCPIDLinear}, nil) {
		t.Fatal("project disable should win")
	}
}

func TestValidateMCPServersRejectsReservedAndDuplicates(t *testing.T) {
	if err := ValidateMCPServers(nil); err != nil {
		t.Fatalf("ValidateMCPServers(nil) = %v, want nil", err)
	}
	if err := ValidateMCPServers([]MCPServer{{ID: "acme", URL: "https://x"}}); err != nil {
		t.Fatalf("ValidateMCPServers(acme) = %v, want nil", err)
	}
	if err := ValidateMCPServers([]MCPServer{{ID: "linear", Enabled: true}}); err != nil {
		t.Fatalf("ValidateMCPServers(linear) = %v, want nil", err)
	}
	if err := ValidateMCPServers([]MCPServer{{ID: "recur"}}); err == nil {
		t.Fatal("ValidateMCPServers(recur) = nil, want reserved error")
	}
	if err := ValidateMCPServers([]MCPServer{{ID: "playwright"}}); err == nil {
		t.Fatal("ValidateMCPServers(playwright) = nil, want reserved error")
	}
	if err := ValidateMCPServers([]MCPServer{{ID: "!!!"}}); err == nil {
		t.Fatal("ValidateMCPServers(!!!) = nil, want invalid error")
	}
	if err := ValidateMCPServers([]MCPServer{
		{ID: "Acme", URL: "https://x"},
		{ID: "acme", URL: "https://y"},
	}); err == nil {
		t.Fatal("ValidateMCPServers(Acme+acme) = nil, want duplicate error")
	}
}

func TestNormalizeDropsReservedAndDedupes(t *testing.T) {
	got := NormalizeMCPServers([]MCPServer{
		{ID: "recur", Enabled: true},
		{ID: "acme", Name: "Acme", URL: "https://x/mcp", Enabled: true},
		{ID: "acme", Name: "Acme 2", URL: "https://y/mcp", Enabled: false},
	})
	if len(got) != 1 || got[0].ID != "acme" || got[0].Enabled || got[0].URL != "https://y/mcp" {
		t.Fatalf("got=%+v", got)
	}
}

func TestLinearMCPURL(t *testing.T) {
	if LinearMCPURL(ChatModeAgent) != LinearMCPWriteURL {
		t.Fatal("agent should get write URL")
	}
	if LinearMCPURL(ChatModeAsk) != LinearMCPReadOnlyURL {
		t.Fatal("ask should get readonly URL")
	}
	if LinearMCPURL(ChatModePlan) != LinearMCPReadOnlyURL {
		t.Fatal("plan should get readonly URL")
	}
	if LinearMCPURL(ChatModeReview) != LinearMCPReadOnlyURL {
		t.Fatal("review should get readonly URL")
	}
	if LinearMCPURL(ChatModeLoop) != LinearMCPWriteURL {
		t.Fatal("loop/maestro should get write URL")
	}
}
