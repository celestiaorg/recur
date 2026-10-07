package codex

import "testing"

func TestFormatPlan(t *testing.T) {
	t.Parallel()
	cases := map[string]string{
		"":                                "",
		"  ":                              "",
		"free":                            "Free",
		"go":                              "Go",
		"plus":                            "Plus",
		"pro":                             "Pro",
		"PRO":                             "Pro",
		"prolite":                         "Pro Lite",
		"team":                            "Team",
		"self_serve_business_prolite":     "Self Serve Business ProLite",
		"self_serve_business_usage_based": "Self Serve Business Usage Based",
		"business":                        "Business",
		"ent26":                           "Enterprise",
		"enterprise":                      "Enterprise",
		"hc":                              "Enterprise",
		"enterprise_cbp_automation":       "Enterprise (Automation)",
		"enterprise_cbp_usage_based":      "Enterprise CBP Usage Based",
		"edu":                             "Edu",
		"education":                       "Edu",
		"edu_plus":                        "Edu Plus",
		"edu_pro":                         "Edu Pro",
		"mystery_plan":                    "mystery_plan",
	}
	for in, want := range cases {
		if got := FormatPlan(in); got != want {
			t.Errorf("FormatPlan(%q) = %q, want %q", in, got, want)
		}
	}
}
