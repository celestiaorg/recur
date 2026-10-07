package engine

import (
	"fmt"
	"math/rand/v2"
	"strings"
)

// Cozy-kitchen workspace ids: "{project-slug}.{adj}-{dish}", with "-N" on the
// kitchen pair on collision (e.g. recur.toasty-bagel, demo.simmered-ramen-2).
// Git worktrees live at ~/.recur/worktrees/<project-slug>/<adj-dish>.
// You're in the app cooking.

var kitchenAdjectives = []string{
	"toasty", "crispy", "buttery", "flaky", "golden", "zesty",
	"simmered", "steamy", "honeyed", "smoky", "herbed", "lemony",
	"velvety", "caramel", "pepper", "roasted", "cozy", "crusty",
	"bubbly", "savory",
}

var kitchenDishes = []string{
	// comfort
	"ramen", "risotto", "chowder", "stew", "curry", "lasagna",
	"potpie", "dumpling", "gratin", "porridge", "pho", "udon",
	"biryani", "goulash",
	// bakery
	"bagel", "scone", "brioche", "galette", "focaccia", "pretzel",
	"tart", "muffin", "biscuit", "croissant", "babka", "ciabatta",
	// egg / savory
	"omelet", "frittata", "hash", "quiche", "toastie", "tacos",
	"burrito", "pierogi",
	// sweet
	"cobbler", "brownie", "parfait", "pudding", "shortcake", "trifle",
	"cannoli", "churro", "macaron", "eclair",
}

const kitchenIDReshuffles = 16

// kitchenPair returns a random "{adj}-{dish}" id with no numeric suffix.
func kitchenPair() string {
	adj := kitchenAdjectives[rand.IntN(len(kitchenAdjectives))]
	dish := kitchenDishes[rand.IntN(len(kitchenDishes))]
	return adj + "-" + dish
}

// allocKitchenID picks a free cozy-kitchen workspace id.
// taken reports whether an id is already in use.
func allocKitchenID(taken func(string) bool) string {
	for i := 0; i < kitchenIDReshuffles; i++ {
		id := kitchenPair()
		if !taken(id) {
			return id
		}
	}
	base := kitchenPair()
	if !taken(base) {
		return base
	}
	for n := 2; ; n++ {
		id := fmt.Sprintf("%s-%d", base, n)
		if !taken(id) {
			return id
		}
	}
}

// composeWorkspaceID builds `{project}.{kitchen}` (e.g. recur.flaky-ramen).
// An empty project slug returns the kitchen id unchanged (legacy / tests).
func composeWorkspaceID(projectID, kitchenSlug string) string {
	projectID = strings.TrimSpace(projectID)
	kitchenSlug = strings.TrimSpace(kitchenSlug)
	if projectID == "" {
		return kitchenSlug
	}
	if kitchenSlug == "" {
		return projectID
	}
	return projectID + "." + kitchenSlug
}
