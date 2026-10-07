package app

import "testing"

func TestInstanceLabel(t *testing.T) {
	t.Run("unset", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "")
		if got := InstanceLabel(); got != "" {
			t.Errorf("InstanceLabel() = %q, want %q", got, "")
		}
	})
	t.Run("whitespace", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "  \t\n")
		if got := InstanceLabel(); got != "" {
			t.Errorf("InstanceLabel() = %q, want %q", got, "")
		}
	})
	t.Run("set", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "  mycodecrafting/smoky-shortcake  ")
		if got := InstanceLabel(); got != "mycodecrafting/smoky-shortcake" {
			t.Errorf("InstanceLabel() = %q, want %q", got, "mycodecrafting/smoky-shortcake")
		}
	})
}

func TestWindowTitle(t *testing.T) {
	t.Run("unset", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "")
		if got := WindowTitle(); got != "Recur" {
			t.Errorf("WindowTitle() = %q, want %q", got, "Recur")
		}
	})
	t.Run("whitespace", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "   ")
		if got := WindowTitle(); got != "Recur" {
			t.Errorf("WindowTitle() = %q, want %q", got, "Recur")
		}
	})
	t.Run("set", func(t *testing.T) {
		t.Setenv(instanceLabelEnv, "mycodecrafting/smoky-shortcake")
		if got := WindowTitle(); got != "mycodecrafting/smoky-shortcake" {
			t.Errorf("WindowTitle() = %q, want %q", got, "mycodecrafting/smoky-shortcake")
		}
	})
}
