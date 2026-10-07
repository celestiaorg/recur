package observe

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"recur/internal/domain"
)

// RunChecks executes available test/typecheck/lint commands in root and returns
// Check rows. Missing toolchains are skipped (not failed).
func RunChecks(ctx context.Context, root string, testResult *domain.TestResult) []domain.Check {
	if ctx == nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(context.Background(), 90*time.Second)
		defer cancel()
	}
	checks := agentTestCheck(testResult)

	if hasGoMod(root) {
		checks = append(checks, runCmdCheck(ctx, root, "Tests", "go", "test", "./...", "-count=1", "-timeout=45s"))
		checks = append(checks, runCmdCheck(ctx, root, "Typecheck (go)", "go", "vet", "./..."))
	}
	if hasPackageJSON(root) {
		if hasScript(root, "test") {
			checks = append(checks, runCmdCheck(ctx, root, "Tests (npm)", "npm", "test", "--", "--runInBand"))
		}
		if hasNodeBin(root, "tsc") || hasDevDep(root, "typescript") {
			checks = append(checks, runCmdCheck(ctx, root, "Typecheck (tsc)", "npx", "--no-install", "tsc", "--noEmit"))
		}
		if hasScript(root, "lint") {
			checks = append(checks, runCmdCheck(ctx, root, "Lint", "npm", "run", "lint"))
		}
	}
	if hasFile(root, "Cargo.toml") {
		checks = append(checks, runCmdCheck(ctx, root, "Typecheck (cargo)", "cargo", "check", "--quiet"))
		checks = append(checks, runCmdCheck(ctx, root, "Tests (cargo)", "cargo", "test", "--quiet", "--", "--test-threads=1"))
	}
	if hasFile(root, "pyproject.toml") || hasFile(root, "setup.py") || hasFile(root, "requirements.txt") {
		if hasBinary("pytest") {
			checks = append(checks, runCmdCheck(ctx, root, "Tests (pytest)", "pytest", "-q", "--tb=no"))
		} else if hasBinary("python") || hasBinary("python3") {
			bin := "python3"
			if !hasBinary("python3") {
				bin = "python"
			}
			checks = append(checks, runCmdCheck(ctx, root, "Compile (python)", bin, "-m", "compileall", "-q", "."))
		}
	}
	if hasFile(root, "Gemfile") && hasBinary("bundle") {
		checks = append(checks, runCmdCheck(ctx, root, "Tests (rspec)", "bundle", "exec", "rspec", "--format", "progress"))
	}
	if (hasFile(root, "pom.xml") || hasFile(root, "build.gradle") || hasFile(root, "build.gradle.kts")) && hasBinary("mvn") {
		if hasFile(root, "pom.xml") {
			checks = append(checks, runCmdCheck(ctx, root, "Compile (maven)", "mvn", "-q", "-DskipTests", "compile"))
		}
	}
	if hasFile(root, "Package.swift") {
		checks = append(checks, runCmdCheck(ctx, root, "Build (swift)", "swift", "build"))
	}
	return checks
}

// RunLightChecks is used at PR-open time: agent test result + a quick typecheck,
// never a full test suite (so the loop cannot hang on openPR).
func RunLightChecks(ctx context.Context, root string, testResult *domain.TestResult) []domain.Check {
	if ctx == nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(context.Background(), 25*time.Second)
		defer cancel()
	}
	checks := agentTestCheck(testResult)
	if hasGoMod(root) {
		checks = append(checks, runCmdCheck(ctx, root, "Typecheck (go)", "go", "vet", "./..."))
	}
	if hasFile(root, "Cargo.toml") {
		checks = append(checks, runCmdCheck(ctx, root, "Typecheck (cargo)", "cargo", "check", "--quiet"))
	}
	if hasPackageJSON(root) && (hasNodeBin(root, "tsc") || hasDevDep(root, "typescript")) {
		checks = append(checks, runCmdCheck(ctx, root, "Typecheck (tsc)", "npx", "--no-install", "tsc", "--noEmit"))
	}
	return checks
}

func agentTestCheck(testResult *domain.TestResult) []domain.Check {
	if testResult == nil {
		return nil
	}
	detail := strings.TrimSpace(testResult.Summary)
	if detail == "" {
		if testResult.Passed {
			detail = "Tester reported pass"
		} else {
			detail = "Tester reported fail"
		}
	}
	return []domain.Check{{Name: "Tests (agent)", Detail: truncate(detail, 120), Passed: testResult.Passed}}
}

func runCmdCheck(ctx context.Context, root, name, bin string, args ...string) domain.Check {
	cctx, cancel := context.WithTimeout(ctx, 50*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, bin, args...)
	cmd.Dir = root
	var buf bytes.Buffer
	cmd.Stdout = &buf
	cmd.Stderr = &buf
	err := cmd.Run()
	out := strings.TrimSpace(buf.String())
	if err != nil {
		detail := firstLine(out)
		if detail == "" {
			detail = err.Error()
		}
		// Missing binary → skip rather than fail the PR.
		if isMissingBinary(err, out) {
			return domain.Check{Name: name, Detail: "skipped (tool unavailable)", Passed: true}
		}
		return domain.Check{Name: name, Detail: truncate(detail, 140), Passed: false}
	}
	detail := "ok"
	if n := countPASS(out); n > 0 {
		detail = fmt.Sprintf("%d passed", n)
	}
	return domain.Check{Name: name, Detail: detail, Passed: true}
}

func isMissingBinary(err error, out string) bool {
	msg := err.Error() + out
	return strings.Contains(msg, "executable file not found") ||
		strings.Contains(msg, "not found") && strings.Contains(msg, "npx")
}

func hasGoMod(root string) bool {
	return hasFile(root, "go.mod")
}

func hasPackageJSON(root string) bool {
	return hasFile(root, "package.json")
}

func hasFile(root, name string) bool {
	_, err := os.Stat(filepath.Join(root, name))
	return err == nil
}

func hasBinary(name string) bool {
	_, err := exec.LookPath(name)
	return err == nil
}

func hasScript(root, name string) bool {
	b, err := os.ReadFile(filepath.Join(root, "package.json"))
	if err != nil {
		return false
	}
	return strings.Contains(string(b), `"`+name+`"`)
}

func hasDevDep(root, name string) bool {
	b, err := os.ReadFile(filepath.Join(root, "package.json"))
	if err != nil {
		return false
	}
	return strings.Contains(string(b), `"`+name+`"`)
}

func hasNodeBin(root, name string) bool {
	_, err := os.Stat(filepath.Join(root, "node_modules", ".bin", name))
	return err == nil
}

func firstLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return strings.TrimSpace(s[:i])
	}
	return s
}

func truncate(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func countPASS(out string) int {
	n := 0
	for _, line := range strings.Split(out, "\n") {
		if strings.HasPrefix(line, "ok\t") || strings.Contains(line, " PASS ") {
			n++
		}
	}
	return n
}
