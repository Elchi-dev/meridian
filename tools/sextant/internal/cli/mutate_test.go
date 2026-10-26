package cli

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
)

const lessGo = "package m\n\nfunc Less(a, b int) bool {\n\treturn a < b\n}\n"

const lessTest = "package m\n\nimport \"testing\"\n\nfunc TestLess(t *testing.T) {\n\tif !Less(1, 2) || Less(2, 1) {\n\t\tt.Fatal(\"wrong\")\n\t}\n}\n"

// A checkout with one Go module whose tests miss one mutant: Less(2, 2)
// is never asked, so < to <= survives.
func mutateRepo() map[string]string {
	return map[string]string{
		"sextant.local.meri": "mutate {\n    scratch = \"../scratch\"\n}\n",
		"m/go.mod":           "module m\n\ngo 1.27.0\n",
		"m/less.go":          lessGo,
		"m/less_test.go":     lessTest,
	}
}

const lessAllow = "allow {\n    file = \"m/less.go\"\n    line = \"return a < b\"\n    mutation = \"< to <=\"\n    reason = \"callers never pass equal values\"\n}\n"

func TestMutateList(t *testing.T) {
	root := checkout(t, mutateRepo())
	r := runIn(root, "mutate", "--list")
	want := "m/less.go:3: body to { return false }\n" +
		"m/less.go:3: body to { return true }\n" +
		"m/less.go:4: < to <=\n" +
		"m/less.go:4: < to >=\n" +
		"4 mutants in 1 file, 0 allowed\n"
	if r.code != ExitOK || r.stdout != want {
		t.Fatalf("exit %d\nstdout\n%s\nstderr %s", r.code, r.stdout, r.stderr)
	}
}

func TestMutateListCountsAllowed(t *testing.T) {
	repo := mutateRepo()
	repo["mutants.meri"] = lessAllow
	r := runIn(checkout(t, repo), "mutate", "--list")
	if r.code != ExitOK || !strings.HasSuffix(r.stdout, "3 mutants in 1 file, 1 allowed\n") || strings.Contains(r.stdout, "< to <=") {
		t.Fatalf("exit %d\n%s", r.code, r.stdout)
	}
}

func TestMutateListReportsStaleAllowances(t *testing.T) {
	repo := mutateRepo()
	repo["mutants.meri"] = strings.Replace(lessAllow, "return a < b", "return a <= b", 1)
	r := runIn(checkout(t, repo), "mutate", "--list")
	if r.code != ExitFailure {
		t.Fatalf("exit %d", r.code)
	}
	want := "mutants.meri:1:1: this allow matches no mutant in m/less.go any more; remove it\nsextant mutate: mutants.meri has 1 stale entry\n"
	if r.stderr != want {
		t.Fatalf("stderr\n%q\nwant\n%q", r.stderr, want)
	}
}

func TestMutateRunFindsTheSurvivor(t *testing.T) {
	root := checkout(t, mutateRepo())
	r := runIn(root, "mutate", "--jobs", "1")
	if r.code != ExitFailure {
		t.Fatalf("exit %d\nstdout %s\nstderr %s", r.code, r.stdout, r.stderr)
	}
	want := "4 mutants in 1 file\n" +
		"  killed         3\n" +
		"  timed out      0\n" +
		"  out of memory  0\n" +
		"  unviable       0\n" +
		"  allowed        0\n" +
		"  survived       1\n" +
		"\nm/less.go:4: < to <=\n" +
		"allow {\n    file = \"m/less.go\"\n    line = \"return a < b\"\n    mutation = \"< to <=\"\n    reason = \"why this change cannot alter behaviour\"\n}\n"
	if r.stdout != want {
		t.Fatalf("stdout\n%s\nwant\n%s", r.stdout, want)
	}
	if !strings.Contains(r.stderr, "survived: m/less.go:4: < to <=\n") ||
		!strings.HasSuffix(r.stderr, "sextant mutate: 1 mutant survived; write a test that catches each, or allow it in mutants.meri with the reason it cannot change behaviour\n") {
		t.Fatalf("stderr\n%s", r.stderr)
	}
}

// A crate with QEMU checks: without them, a boundary its host tests miss
// survives, and is reported as one.
func TestMutateCanLeaveTheMachineChecksOut(t *testing.T) {
	needTool(t, "cargo")
	root := checkout(t, map[string]string{
		"sextant.local.meri": "mutate {\n    scratch = \"../scratch\"\n}\n",
		"k/Cargo.toml":       "[package]\nname = \"k\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[features]\nqemu-test = []\n",
		"k/src/lib.rs":       "pub fn less(a: u8, b: u8) -> bool {\n    a < b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn less() {\n        assert!(super::less(1, 2) && !super::less(3, 2));\n    }\n}\n",
	})
	r := runIn(root, "mutate", "--machine=false", "--jobs", "1")
	if r.code != ExitFailure || !strings.Contains(r.stdout, "k/src/lib.rs:2: < to <=\n") || !strings.Contains(r.stdout, "  survived       1\n") {
		t.Fatalf("exit %d\nstdout %s\nstderr %s", r.code, r.stdout, r.stderr)
	}
	bad := runIn(root, "mutate", "--machine=sometimes")
	if bad.code != ExitUsage || !strings.Contains(bad.stderr, "invalid boolean value") {
		t.Fatalf("exit %d, stderr %q", bad.code, bad.stderr)
	}
}

func TestMutateRunPassesWhenEverySurvivorIsAllowed(t *testing.T) {
	repo := mutateRepo()
	repo["mutants.meri"] = lessAllow
	r := runIn(checkout(t, repo), "mutate", "--jobs", "2")
	if r.code != ExitOK || !strings.HasPrefix(r.stdout, "4 mutants in 1 file\n") || !strings.Contains(r.stdout, "  allowed        1\n") || !strings.Contains(r.stdout, "  survived       0\n") {
		t.Fatalf("exit %d\nstdout %s\nstderr %s", r.code, r.stdout, r.stderr)
	}
}

func TestMutateRunStillReportsStaleAllowances(t *testing.T) {
	repo := mutateRepo()
	repo["mutants.meri"] = lessAllow + strings.Replace(lessAllow, "< to <=", "< to >", 1)
	repo["m/less_test.go"] = strings.Replace(lessTest, "Less(2, 1)", "Less(2, 1) || Less(2, 2)", 1)
	r := runIn(checkout(t, repo), "mutate")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "mutants.meri:7:1: this allow matches no mutant in m/less.go any more") {
		t.Fatalf("exit %d\nstderr %s", r.code, r.stderr)
	}
}

func TestMutateSettingsFromTheCommandLine(t *testing.T) {
	m, err := parseMutate(nil)
	if err != nil || m.since != "" || m.jobs != 2 || m.list || !m.machine || len(m.paths) != 0 {
		t.Fatalf("defaults: %+v, %v", m, err)
	}
	m, err = parseMutate([]string{"lib/fat", "--since", "main", "--jobs", "4", "kernel", "--list", "--machine=false"})
	if err != nil || m.since != "main" || m.jobs != 4 || !m.list || m.machine || !slices.Equal(m.paths, []string{"lib/fat", "kernel"}) {
		t.Fatalf("all set: %+v, %v", m, err)
	}
	if m, err := parseMutate([]string{"--jobs", "1"}); err != nil || m.jobs != 1 {
		t.Fatalf("one at a time: %+v, %v", m, err)
	}
}

func TestMutateFlagsMayFollowPaths(t *testing.T) {
	root := checkout(t, mutateRepo())
	r := runIn(filepath.Join(root, "m"), "mutate", "less.go", "--jobs", "3", "--list")
	if r.code != ExitOK || !strings.HasSuffix(r.stdout, "4 mutants in 1 file, 0 allowed\n") {
		t.Fatalf("exit %d\n%s%s", r.code, r.stdout, r.stderr)
	}
}

func TestMutateUsageErrors(t *testing.T) {
	root := checkout(t, mutateRepo())
	tests := []struct {
		args []string
		code int
		want string
	}{
		{[]string{"mutate", "--jobs", "0"}, ExitUsage, "sextant mutate: --jobs must be at least 1, got 0\n"},
		{[]string{"mutate", "--fast"}, ExitUsage, "sextant mutate: flag provided but not defined: -fast\n"},
		{[]string{"mutate", "docs", "--list"}, ExitFailure, "sextant mutate: docs has no Rust or Go code to mutate\n"},
		{[]string{"mutate", "../elsewhere", "--list"}, ExitFailure, "is outside the repository at"},
		{[]string{"mutate", "--list", filepath.Dir(root)}, ExitFailure, "is outside the repository at"},
	}
	for _, tt := range tests {
		r := runIn(root, tt.args...)
		if r.code != tt.code || !strings.Contains(r.stderr, tt.want) {
			t.Errorf("%v: exit %d, stderr %q", tt.args, r.code, r.stderr)
		}
	}
}

func TestMutateOutsideACheckout(t *testing.T) {
	r := runIn(t.TempDir(), "mutate", "--list")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "not inside a Meridian checkout") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestMutateBrokenAllowList(t *testing.T) {
	repo := mutateRepo()
	repo["mutants.meri"] = "allow {\n"
	r := runIn(checkout(t, repo), "mutate", "--list")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "mutants.meri:") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestMutateBrokenSource(t *testing.T) {
	repo := mutateRepo()
	repo["m/broken.go"] = "package m\n\nfunc (\n"
	r := runIn(checkout(t, repo), "mutate", "--list")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "m/broken.go: ") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"}, args...)...)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func TestMutateSince(t *testing.T) {
	repo := mutateRepo()
	repo["m/more.go"] = "package m\n\nvar Big = 3 > 2\n"
	root := checkout(t, repo)
	git(t, root, "init", "-q")
	git(t, root, "add", ".")
	git(t, root, "commit", "-q", "-m", "start")
	if err := os.WriteFile(filepath.Join(root, "m", "more.go"), []byte("package m\n\nvar Big = 3 >= 2\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	r := runIn(root, "mutate", "--since", "HEAD", "--list")
	want := "m/more.go:3: >= to <\nm/more.go:3: >= to >\n2 mutants in 1 file, 0 allowed\n"
	if r.code != ExitOK || r.stdout != want {
		t.Fatalf("exit %d\n%s%s", r.code, r.stdout, r.stderr)
	}
	r = runIn(root, "mutate", "--since", "no-such-ref", "--list")
	if r.code != ExitFailure || !strings.HasPrefix(r.stderr, "sextant mutate: git cannot tell what changed since no-such-ref: exit status ") || !strings.Contains(r.stderr, "fatal: bad revision 'no-such-ref'") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestMutateSinceOutsideGit(t *testing.T) {
	root := checkout(t, mutateRepo())
	r := runIn(root, "mutate", "--since", "HEAD", "--list")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "git cannot tell what changed since HEAD") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestRepoPaths(t *testing.T) {
	root := filepath.FromSlash("/r/meridian")
	if runtime.GOOS == "windows" {
		root = `C:\r\meridian`
	}
	got, err := repoPaths(root, filepath.Join(root, "tools"), []string{"sextant", filepath.Join(root, "lib"), "..", "."})
	if err != nil || strings.Join(got, " ") != "tools/sextant lib . tools" {
		t.Fatalf("%v %v", got, err)
	}
	for _, p := range []string{"../../x", "../.."} {
		if _, err := repoPaths(root, filepath.Join(root, "tools"), []string{p}); err == nil {
			t.Errorf("%s leaves the repository", p)
		}
	}
	if _, err := repoPaths(root, root, []string{"..x"}); err != nil {
		t.Errorf("a folder named ..x is inside: %v", err)
	}
}

func TestProgressEveryTwentyFive(t *testing.T) {
	tests := map[int]string{1: "", 24: "", 25: "mutate: 25 of 60 done\n", 26: "", 50: "mutate: 50 of 60 done\n", 0: "mutate: 0 of 60 done\n"}
	for done, want := range tests {
		if got := progress(done, 60); got != want {
			t.Errorf("progress(%d) = %q, want %q", done, got, want)
		}
	}
}

// isolateCache points the user's cache folder at a temporary one, so tests
// never write into the real cache. Only for tests that start no go command:
// Go keeps its build cache there too, and an empty one makes every build
// start from scratch.
func isolateCache(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	for _, v := range []string{"XDG_CACHE_HOME", "HOME", "LocalAppData"} {
		t.Setenv(v, dir)
	}
	cache, err := os.UserCacheDir()
	if err != nil {
		t.Fatal(err)
	}
	return cache
}

func scratchOf(t *testing.T, root string) (string, error) {
	t.Helper()
	settings, err := config.Load(root)
	if err != nil {
		t.Fatal(err)
	}
	return mutateScratch(settings)
}

// The default build folder, target, lies inside the repository, so without
// settings of its own a run goes to the cache folder.
func TestMutateScratchDefaultsToTheCacheFolder(t *testing.T) {
	cache := isolateCache(t)
	for _, local := range []string{"", "build {\n    dir = \"out\"\n}\n"} {
		repo := mutateRepo()
		repo["sextant.local.meri"] = local
		dir, err := scratchOf(t, checkout(t, repo))
		if err != nil || dir != filepath.Join(cache, "sextant", "mutate") {
			t.Fatalf("%q: %q %v", local, dir, err)
		}
		if info, err := os.Stat(dir); err != nil || !info.IsDir() {
			t.Fatalf("the folder is created: %v", err)
		}
	}
}

func TestMutateScratchFollowsTheSettings(t *testing.T) {
	isolateCache(t)
	elsewhere := t.TempDir()
	tests := []struct {
		local, want string
	}{
		{"build {\n    dir = \"" + filepath.ToSlash(elsewhere) + "\"\n}\n", filepath.Join(elsewhere, "mutate")},
		{"mutate {\n    scratch = \"" + filepath.ToSlash(elsewhere) + "/m\"\n}\nbuild {\n    dir = \"/x\"\n}\n", filepath.Join(elsewhere, "m")},
		{"mutate {\n    scratch = \"" + filepath.ToSlash(elsewhere) + "/n\"\n}\n", filepath.Join(elsewhere, "n")},
	}
	for _, tt := range tests {
		repo := mutateRepo()
		repo["sextant.local.meri"] = tt.local
		dir, err := scratchOf(t, checkout(t, repo))
		if err != nil || dir != tt.want {
			t.Errorf("%q: %q %v, want %q", tt.local, dir, err, tt.want)
		}
	}
}

func TestMutateScratchMustBeOutsideTheRepository(t *testing.T) {
	isolateCache(t)
	for _, local := range []string{
		"mutate {\n    scratch = \"in/repo\"\n}\n",
		"mutate {\n    scratch = \".\"\n}\n",
	} {
		repo := mutateRepo()
		repo["sextant.local.meri"] = local
		_, err := scratchOf(t, checkout(t, repo))
		if err == nil || !strings.Contains(err.Error(), "is inside the repository; point mutate.scratch") {
			t.Errorf("%q: %v", local, err)
		}
	}
	repo := mutateRepo()
	repo["sextant.local.meri"] = "mutate {\n    scratch = \"../beside\"\n}\n"
	root := checkout(t, repo)
	if dir, err := scratchOf(t, root); err != nil || dir != filepath.Join(filepath.Dir(root), "beside") {
		t.Errorf("a folder beside the repository is fine: %q %v", dir, err)
	}
}

func TestMutateScratchNeedsACacheFolderOrASetting(t *testing.T) {
	for _, v := range []string{"XDG_CACHE_HOME", "HOME", "LocalAppData", "home"} {
		t.Setenv(v, "")
	}
	repo := mutateRepo()
	repo["sextant.local.meri"] = ""
	_, err := scratchOf(t, checkout(t, repo))
	if err == nil || !strings.Contains(err.Error(), "there is no cache folder for mutation runs") {
		t.Fatalf("got %v", err)
	}
}

func TestMutateRefusesBrokenSettings(t *testing.T) {
	repo := mutateRepo()
	repo["sextant.local.meri"] = "mutate {\n    scratch = true\n}\n"
	r := runIn(checkout(t, repo), "mutate", "m/less.go")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "scratch") {
		t.Fatalf("exit %d: %s", r.code, r.stderr)
	}
}

func TestMutateRunUsesTheScratchFolder(t *testing.T) {
	repo := mutateRepo()
	scratch := filepath.Join(t.TempDir(), "s")
	repo["sextant.local.meri"] = "mutate {\n    scratch = \"" + filepath.ToSlash(scratch) + "\"\n}\n"
	root := checkout(t, repo)
	r := runIn(root, "mutate", "m/less.go")
	if r.code != ExitFailure {
		t.Fatalf("exit %d, %s", r.code, r.stderr)
	}
	entries, err := os.ReadDir(scratch)
	if err != nil {
		t.Fatalf("the scratch folder from the settings was not used: %v", err)
	}
	if len(entries) != 0 {
		t.Fatalf("a finished run leaves its scratch folder empty, found %d entries", len(entries))
	}
	if _, err := os.Stat(filepath.Join(root, "out")); !os.IsNotExist(err) {
		t.Fatalf("with mutate.scratch set, out/ is not used: %v", err)
	}
}
