package main

import "testing"

func TestParseTitlebarDoubleClickAction(t *testing.T) {
	cases := []struct {
		in   string
		want titlebarClickAction
	}{
		{"", titlebarActionZoom},
		{"Maximize", titlebarActionZoom},
		{"maximize", titlebarActionZoom},
		{"Fill", titlebarActionZoom},
		{"FILL", titlebarActionZoom},
		{"Minimize", titlebarActionMinimize},
		{"  minimize  ", titlebarActionMinimize},
		{"None", titlebarActionNone},
		{"none", titlebarActionNone},
		{"bogus", titlebarActionZoom},
	}
	for _, tc := range cases {
		if got := parseTitlebarDoubleClickAction(tc.in); got != tc.want {
			t.Fatalf("parseTitlebarDoubleClickAction(%q) = %d, want %d", tc.in, got, tc.want)
		}
	}
}
