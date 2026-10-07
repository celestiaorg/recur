package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

type snapshotFetchResult struct {
	elapsed time.Duration
	bytes   int
	cost    int
	err     error
}

func liveBenchPR(t *testing.T) (owner, repo string, number int) {
	t.Helper()
	if os.Getenv("RECUR_GH_BENCH") != "1" {
		t.Skip("set RECUR_GH_BENCH=1 to compare GraphQL vs parallel gh snapshot fetches")
	}
	if !ghCLIAvailable() {
		t.Skip("gh not available/authenticated")
	}
	spec := strings.TrimSpace(os.Getenv("RECUR_GH_BENCH_PR"))
	if spec == "" {
		spec = "cli/cli#14347"
	}
	owner, repo, number, err := parseBenchPR(spec)
	if err != nil {
		t.Fatalf("parseBenchPR(%q) = %v", spec, err)
	}
	return owner, repo, number
}

func parseBenchPR(spec string) (owner, repo string, number int, err error) {
	spec = strings.TrimSpace(spec)
	hash := strings.LastIndexByte(spec, '#')
	slash := strings.IndexByte(spec, '/')
	if hash <= 0 || slash <= 0 || slash > hash {
		return "", "", 0, fmt.Errorf("want owner/repo#N")
	}
	owner, repo = spec[:slash], spec[slash+1:hash]
	number, err = strconv.Atoi(spec[hash+1:])
	if err != nil || owner == "" || repo == "" || number <= 0 {
		return "", "", 0, fmt.Errorf("want owner/repo#N")
	}
	return owner, repo, number, nil
}

func TestParseBenchPR(t *testing.T) {
	o, r, n, err := parseBenchPR("cli/cli#14347")
	if err != nil || o != "cli" || r != "cli" || n != 14347 {
		t.Errorf("parseBenchPR(cli/cli#14347) = %q %q %d %v, want cli cli 14347 nil", o, r, n, err)
	}
	if _, _, _, err := parseBenchPR("nope"); err == nil {
		t.Error("parseBenchPR(nope) error = nil, want error")
	}
}

func TestLiveGitHubGraphQLOmitsCopilotWorkEvents(t *testing.T) {
	owner, repo, number := liveBenchPR(t)
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()

	schema, err := runGH(ctx, "", "api", "graphql", "-f", "query="+
		`{ started: __type(name:"CopilotWorkStartedEvent") { name }
		   finished: __type(name:"CopilotWorkFinishedEvent") { name }
		   enum: __type(name:"PullRequestTimelineItemsItemType") { enumValues { name } }
		   union: __type(name:"PullRequestTimelineItems") { possibleTypes { name } } }`)
	if err != nil {
		t.Fatalf("schema probe: %v", err)
	}
	var env struct {
		Data struct {
			Started  *struct{ Name string } `json:"started"`
			Finished *struct{ Name string } `json:"finished"`
			Enum     struct {
				EnumValues []struct{ Name string } `json:"enumValues"`
			} `json:"enum"`
			Union struct {
				PossibleTypes []struct{ Name string } `json:"possibleTypes"`
			} `json:"union"`
		} `json:"data"`
	}
	if json.Unmarshal([]byte(schema), &env) != nil {
		t.Fatalf("schema probe json = %s", schema)
	}
	if env.Data.Started != nil {
		t.Errorf("__type(CopilotWorkStartedEvent) = %s, want null", env.Data.Started.Name)
	}
	if env.Data.Finished != nil {
		t.Errorf("__type(CopilotWorkFinishedEvent) = %s, want null", env.Data.Finished.Name)
	}
	for _, v := range env.Data.Enum.EnumValues {
		if strings.Contains(strings.ToUpper(v.Name), "COPILOT") {
			t.Errorf("PullRequestTimelineItemsItemType includes %s", v.Name)
		}
	}
	for _, v := range env.Data.Union.PossibleTypes {
		if strings.Contains(strings.ToLower(v.Name), "copilot") {
			t.Errorf("PullRequestTimelineItems includes %s", v.Name)
		}
	}

	tlOut, err := runGH(ctx, "", "api",
		fmt.Sprintf("repos/%s/%s/issues/%d/timeline?per_page=100", owner, repo, number),
		"-H", "Accept: application/vnd.github+json")
	if err != nil {
		t.Fatalf("REST timeline: %v", err)
	}
	var rest []struct {
		Event  string `json:"event"`
		NodeID string `json:"node_id"`
	}
	if json.Unmarshal([]byte(tlOut), &rest) != nil {
		t.Fatalf("REST timeline json")
	}
	var copilot int
	var copilotID string
	for _, ev := range rest {
		if strings.HasPrefix(ev.Event, "copilot_work") {
			copilot++
			if copilotID == "" {
				copilotID = ev.NodeID
			}
		}
	}
	if copilot == 0 {
		t.Skipf("%s/%s#%d REST timeline has no copilot_work_* events", owner, repo, number)
	}

	gqlOut, err := runGH(ctx, "", "api", "graphql", "-f", "query="+
		`query($o:String!,$r:String!,$n:Int!){
		  repository(owner:$o,name:$r){ pullRequest(number:$n){
		    timelineItems(first:100){ totalCount filteredCount nodes { __typename } } } } }`,
		"-F", "o="+owner, "-F", "r="+repo, "-F", "n="+strconv.Itoa(number))
	if err != nil {
		t.Fatalf("GraphQL timeline: %v", err)
	}
	var gql struct {
		Data struct {
			Repository struct {
				PullRequest struct {
					TimelineItems struct {
						TotalCount    int `json:"totalCount"`
						FilteredCount int `json:"filteredCount"`
						Nodes         []struct {
							Typename string `json:"__typename"`
						} `json:"nodes"`
					} `json:"timelineItems"`
				} `json:"pullRequest"`
			} `json:"repository"`
		} `json:"data"`
	}
	if json.Unmarshal([]byte(gqlOut), &gql) != nil {
		t.Fatalf("GraphQL timeline json = %s", gqlOut)
	}
	items := gql.Data.Repository.PullRequest.TimelineItems
	for _, n := range items.Nodes {
		if strings.Contains(strings.ToLower(n.Typename), "copilot") {
			t.Errorf("timelineItems node __typename = %s, want no Copilot types", n.Typename)
		}
	}
	if items.FilteredCount >= items.TotalCount {
		t.Errorf("timelineItems filteredCount=%d totalCount=%d, want filteredCount < totalCount (hidden Copilot items)", items.FilteredCount, items.TotalCount)
	}
	hidden := items.TotalCount - items.FilteredCount
	if hidden < copilot {
		t.Errorf("GraphQL hid %d timeline items, REST copilot_work_* = %d, want hidden >= REST copilot count", hidden, copilot)
	}
	t.Logf("REST copilot_work_* = %d; GraphQL totalCount=%d filteredCount=%d hidden=%d", copilot, items.TotalCount, items.FilteredCount, hidden)

	gqlSnap, err := runGH(ctx, "", "api", "graphql",
		"-f", "query="+githubPRSnapshotGraphQL,
		"-F", "o="+owner, "-F", "r="+repo, "-F", "n="+strconv.Itoa(number))
	if err != nil {
		t.Fatalf("hybrid GraphQL snapshot: %v", err)
	}
	snap, err := parseGitHubPRSnapshotGraphQL(gqlSnap, time.Now())
	if err != nil {
		t.Fatalf("parseGitHubPRSnapshotGraphQL() error = %v, want nil", err)
	}
	snap = finishGitHubPRSnapshot(snap, tlOut, time.Now())
	var gotCopilot int
	for _, c := range snap.Comments {
		if strings.Contains(c.BodyMD, "on behalf of") {
			gotCopilot++
		}
	}
	if gotCopilot != copilot {
		t.Errorf("hybrid snapshot Copilot actions = %d, want %d REST copilot_work_* events", gotCopilot, copilot)
	}

	if copilotID != "" {
		nodeOut, nodeErr := runGH(ctx, "", "api", "graphql",
			"-f", "query=query($id:ID!){ node(id:$id) { __typename } }",
			"-F", "id="+copilotID)
		if nodeErr == nil {
			t.Errorf("node(%s) succeeded = %s, want GraphQL error (CWSE ids are not in the public schema)", copilotID, nodeOut)
		}
	}
}

func TestLiveGitHubSnapshotEfficiency(t *testing.T) {
	owner, repo, number := liveBenchPR(t)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()

	_ = fetchSnapshotGraphQL(ctx, owner, repo, number) // warmup
	_ = fetchSnapshotHybrid(ctx, owner, repo, number)
	_ = fetchSnapshotParallel(ctx, owner, repo, number)

	const rounds = 5
	var gql, hyb, par []snapshotFetchResult
	for i := 0; i < rounds; i++ {
		gql = append(gql, fetchSnapshotGraphQL(ctx, owner, repo, number))
		hyb = append(hyb, fetchSnapshotHybrid(ctx, owner, repo, number))
		par = append(par, fetchSnapshotParallel(ctx, owner, repo, number))
	}
	for i, r := range gql {
		if r.err != nil {
			t.Fatalf("fetchSnapshotGraphQL[%d] error = %v", i, r.err)
		}
	}
	for i, r := range hyb {
		if r.err != nil {
			t.Fatalf("fetchSnapshotHybrid[%d] error = %v", i, r.err)
		}
	}
	for i, r := range par {
		if r.err != nil {
			t.Fatalf("fetchSnapshotParallel[%d] error = %v", i, r.err)
		}
	}
	gMed, gMean := summarizeElapsed(gql)
	hMed, hMean := summarizeElapsed(hyb)
	pMed, pMean := summarizeElapsed(par)
	t.Logf("%s/%s#%d  n=%d", owner, repo, number, rounds)
	t.Logf("one GraphQL:       median %s  mean %s  %d B  cost %d", gMed, gMean, gql[0].bytes, gql[0].cost)
	t.Logf("GraphQL+REST:      median %s  mean %s  %d B", hMed, hMean, hyb[0].bytes)
	t.Logf("parallel gh:       median %s  mean %s", pMed, pMean)
	if hMed > pMed {
		t.Logf("hybrid was slower on this PR (median %s vs parallel %s)", hMed, pMed)
	}
}

func summarizeElapsed(rs []snapshotFetchResult) (median, mean time.Duration) {
	if len(rs) == 0 {
		return 0, 0
	}
	xs := make([]time.Duration, len(rs))
	var sum time.Duration
	for i, r := range rs {
		xs[i] = r.elapsed
		sum += r.elapsed
	}
	for i := 1; i < len(xs); i++ {
		for j := i; j > 0 && xs[j] < xs[j-1]; j-- {
			xs[j], xs[j-1] = xs[j-1], xs[j]
		}
	}
	return xs[len(xs)/2], sum / time.Duration(len(xs))
}

func fetchSnapshotGraphQL(ctx context.Context, owner, repo string, number int) snapshotFetchResult {
	start := time.Now()
	out, err := runGH(ctx, "", "api", "graphql",
		"-f", "query="+githubPRSnapshotGraphQL,
		"-F", "o="+owner,
		"-F", "r="+repo,
		"-F", "n="+strconv.Itoa(number))
	r := snapshotFetchResult{elapsed: time.Since(start), bytes: len(out), err: err}
	if err != nil {
		return r
	}
	if _, err := parseGitHubPRSnapshotGraphQL(out, time.Now()); err != nil {
		r.err = err
		return r
	}
	var payload struct {
		Data struct {
			RateLimit struct {
				Cost int `json:"cost"`
			} `json:"rateLimit"`
		} `json:"data"`
		Errors json.RawMessage `json:"errors"`
	}
	if json.Unmarshal([]byte(out), &payload) != nil {
		r.err = fmt.Errorf("parse graphql envelope")
		return r
	}
	if len(payload.Errors) > 0 && string(payload.Errors) != "null" {
		r.err = fmt.Errorf("graphql errors: %s", payload.Errors)
		return r
	}
	r.cost = payload.Data.RateLimit.Cost
	return r
}

func fetchSnapshotHybrid(ctx context.Context, owner, repo string, number int) snapshotFetchResult {
	start := time.Now()
	var gqlOut, tlOut string
	var gqlErr, tlErr error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		gqlOut, gqlErr = runGH(ctx, "", "api", "graphql",
			"-f", "query="+githubPRSnapshotGraphQL,
			"-F", "o="+owner,
			"-F", "r="+repo,
			"-F", "n="+strconv.Itoa(number))
	}()
	go func() {
		defer wg.Done()
		tlOut, tlErr = runGH(ctx, "", "api",
			fmt.Sprintf("repos/%s/%s/issues/%d/timeline?per_page=100", owner, repo, number),
			"-H", "Accept: application/vnd.github+json")
	}()
	wg.Wait()
	r := snapshotFetchResult{elapsed: time.Since(start), bytes: len(gqlOut) + len(tlOut), err: gqlErr}
	if gqlErr != nil {
		return r
	}
	snap, err := parseGitHubPRSnapshotGraphQL(gqlOut, time.Now())
	if err != nil {
		r.err = err
		return r
	}
	if tlErr != nil {
		r.err = tlErr
		return r
	}
	snap = finishGitHubPRSnapshot(snap, tlOut, time.Now())
	if len(snap.Comments) == 0 {
		r.err = fmt.Errorf("hybrid snapshot comments = 0, want conversation rows")
	}
	return r
}

func fetchSnapshotParallel(ctx context.Context, owner, repo string, number int) snapshotFetchResult {
	ownerRepo := owner + "/" + repo
	n := strconv.Itoa(number)
	jsonFields := "id,title,body,state,author,createdAt,updatedAt,reviewDecision,mergeable,mergeStateStatus,isDraft,comments,reviews,commits,url,headRefOid,headRefName,baseRefName,reviewRequests,assignees"
	start := time.Now()
	view, err := runGH(ctx, "", "pr", "view", n, "--repo", ownerRepo, "--json", jsonFields)
	if err != nil {
		return snapshotFetchResult{elapsed: time.Since(start), err: err}
	}
	total := len(view)
	var mu sync.Mutex
	var first error
	add := func(n int, e error) {
		mu.Lock()
		defer mu.Unlock()
		total += n
		if first == nil && e != nil {
			first = e
		}
	}
	var wg sync.WaitGroup
	wg.Add(6)
	go func() {
		defer wg.Done()
		out, e := runGH(ctx, "", "api", fmt.Sprintf("repos/%s/pulls/%d/comments", ownerRepo, number))
		add(len(out), e)
	}()
	go func() {
		defer wg.Done()
		threads, e := fetchGitHubReviewThreads(ctx, "", ownerRepo, number)
		add(len(threads), e)
	}()
	go func() {
		defer wg.Done()
		meta, avatars := fetchGitHubCommitRollup(ctx, "", ownerRepo, number)
		add(len(meta)+len(avatars), nil)
	}()
	go func() {
		defer wg.Done()
		out, e := runGH(ctx, "", "api",
			fmt.Sprintf("repos/%s/issues/%d/timeline?per_page=100", ownerRepo, number),
			"-H", "Accept: application/vnd.github+json")
		add(len(out), e)
	}()
	go func() {
		defer wg.Done()
		v := githubViewerFn(ctx, "", ownerRepo)
		add(len(v.Login), nil)
	}()
	go func() {
		defer wg.Done()
		hits, _, e := fetchGitHubReactions(ctx, "", ownerRepo, number)
		add(len(hits), e)
	}()
	wg.Wait()
	return snapshotFetchResult{elapsed: time.Since(start), bytes: total, err: first}
}
