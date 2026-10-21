package cli

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
	"syscall"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/mutate"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/project"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/stats"
)

func runMutate(env Env, args []string) error {
	flags := flag.NewFlagSet("mutate", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	since := flags.String("since", "", "only files changed since this git ref")
	jobs := flags.Int("jobs", 2, "mutants tested at the same time")
	list := flags.Bool("list", false, "print the mutants without running them")
	machine := flags.Bool("machine", true, "boot the QEMU test image for Rust mutants the host tests miss in crates with QEMU checks")
	paths, err := parseInterleaved(flags, args)
	if err != nil {
		return usageErrorf("%v", err)
	}
	if *jobs < 1 {
		return usageErrorf("--jobs must be at least 1, got %d", *jobs)
	}
	root, err := project.FindRoot(env.Dir)
	if err != nil {
		return err
	}
	rel, err := repoPaths(root, env.Dir, paths)
	if err != nil {
		return err
	}
	var changed map[string]bool
	if *since != "" {
		if changed, err = changedSince(root, *since); err != nil {
			return err
		}
	}
	sources, err := stats.Sources(os.DirFS(root))
	if err != nil {
		return err
	}
	picked, err := mutate.Pick(sources, rel, changed)
	if err != nil {
		return err
	}
	mutants, targets, err := mutate.Gather(root, picked)
	if err != nil {
		return err
	}
	allow, err := mutate.LoadAllowances(root)
	if err != nil {
		return err
	}
	checked := map[string]bool{}
	for _, s := range picked {
		checked[s.Path] = true
	}
	run, allowed, stale := mutate.Split(allow, mutants, checked)

	if *list {
		for _, m := range run {
			fmt.Fprintln(env.Stdout, m)
		}
		fmt.Fprintf(env.Stdout, "%d %s in %d %s, %d allowed\n",
			len(run), plural(len(run), "mutant", "mutants"), len(picked), plural(len(picked), "file", "files"), len(allowed))
		return staleError(env, stale)
	}

	settings, err := config.Load(root)
	if err != nil {
		return err
	}
	scratch, err := mutateScratch(settings)
	if err != nil {
		return err
	}
	// Interrupt and terminate cancel the run, which ends every test still
	// running; a killed sextant cannot, and leaves them to finish alone.
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	var mu sync.Mutex
	done := 0
	var machineChecks []string
	if *machine {
		self, err := os.Executable()
		if err != nil {
			return fmt.Errorf("find sextant itself to run the QEMU checks: %w", err)
		}
		machineChecks = []string{self, "test", "--qemu"}
	}
	runner := &mutate.Runner{Root: root, Jobs: *jobs, Scratch: scratch, Machine: machineChecks, Done: func(r mutate.Result) {
		mu.Lock()
		defer mu.Unlock()
		done++
		if r.Outcome != mutate.Killed {
			fmt.Fprintf(env.Stderr, "%s: %s\n", r.Outcome, r.Mutant)
		}
		fmt.Fprint(env.Stderr, progress(done, len(run)))
	}}
	results, err := runner.Run(ctx, run, targets)
	if err != nil {
		return err
	}
	survivors := writeMutateSummary(env.Stdout, results, len(allowed), len(picked))
	if err := staleError(env, stale); err != nil {
		return err
	}
	if survivors > 0 {
		return fmt.Errorf("%d %s survived; write a test that catches each, or allow it in %s with the reason it cannot change behaviour",
			survivors, plural(survivors, "mutant", "mutants"), mutate.AllowFile)
	}
	return nil
}

// mutateScratch is where mutation runs keep their copies, builds and
// temporary folders: mutate.scratch from the settings, else the build
// folder with mutate added, else sextant/mutate in the user's cache folder.
// It must lie outside the repository, because the tests' temporary folders
// go there and some tests rely on those not being inside a checkout. For
// the same reason a build folder inside the repository, such as the default
// target, is passed over. The system's temporary folder is never used: on
// many machines it is RAM, and a mutant that loops while writing an image
// would fill it.
func mutateScratch(settings config.Config) (string, error) {
	dir := settings.MutateScratch
	if dir == "" && !within(settings.Root, settings.BuildDir) {
		dir = filepath.Join(settings.BuildDir, "mutate")
	}
	if dir == "" {
		cache, err := os.UserCacheDir()
		if err != nil {
			return "", fmt.Errorf("there is no cache folder for mutation runs (%v); set mutate.scratch in sextant.local.meri", err)
		}
		dir = filepath.Join(cache, "sextant", "mutate")
	}
	if within(settings.Root, dir) {
		return "", fmt.Errorf("the mutation scratch folder %s is inside the repository; point mutate.scratch in sextant.local.meri at a folder outside it", dir)
	}
	return dir, os.MkdirAll(dir, 0o755)
}

// within reports whether p is root or lies below it.
func within(root, p string) bool {
	rel, err := filepath.Rel(root, p)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

// parseInterleaved accepts flags before, between and after the paths.
func parseInterleaved(flags *flag.FlagSet, args []string) ([]string, error) {
	var paths []string
	for {
		if err := flags.Parse(args); err != nil {
			return nil, err
		}
		args = flags.Args()
		if len(args) == 0 {
			return paths, nil
		}
		paths = append(paths, args[0])
		args = args[1:]
	}
}

// repoPaths turns paths given relative to dir into slash paths relative to
// the repository root.
func repoPaths(root, dir string, paths []string) ([]string, error) {
	out := make([]string, 0, len(paths))
	for _, p := range paths {
		if !filepath.IsAbs(p) {
			p = filepath.Join(dir, p)
		}
		rel, err := filepath.Rel(root, p)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return nil, fmt.Errorf("%s is outside the repository at %s", p, root)
		}
		out = append(out, filepath.ToSlash(rel))
	}
	return out, nil
}

// progress is the line to print after done of total mutants: one every 25,
// so a long run shows it is alive without a line per mutant.
func progress(done, total int) string {
	if done%25 != 0 {
		return ""
	}
	return fmt.Sprintf("mutate: %d of %d done\n", done, total)
}

func changedSince(root, ref string) (map[string]bool, error) {
	var stderr strings.Builder
	cmd := exec.Command("git", "-C", root, "diff", "--name-only", ref, "--")
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("git cannot tell what changed since %s: %v: %s", ref, err, strings.TrimSpace(stderr.String()))
	}
	changed := map[string]bool{}
	for _, line := range strings.Split(string(out), "\n") {
		if line != "" {
			changed[line] = true
		}
	}
	return changed, nil
}

func staleError(env Env, stale []mutate.Allowance) error {
	for _, a := range stale {
		fmt.Fprintf(env.Stderr, "%s: this allow matches no mutant in %s any more; remove it\n", a.Pos, a.File)
	}
	if len(stale) > 0 {
		return fmt.Errorf("%s has %d stale %s", mutate.AllowFile, len(stale), plural(len(stale), "entry", "entries"))
	}
	return nil
}

// writeMutateSummary prints the counts and every survivor with the allow
// block to paste, and returns the number of survivors.
func writeMutateSummary(w io.Writer, results []mutate.Result, allowed, files int) int {
	counts := map[mutate.Outcome]int{}
	var survivors []mutate.Mutant
	for _, r := range results {
		counts[r.Outcome]++
		if r.Outcome == mutate.Survived {
			survivors = append(survivors, r.Mutant)
		}
	}
	total := len(results) + allowed
	fmt.Fprintf(w, "%d %s in %d %s\n", total, plural(total, "mutant", "mutants"), files, plural(files, "file", "files"))
	writeTable(w, [][]string{
		{"  killed", fmt.Sprint(counts[mutate.Killed])},
		{"  timed out", fmt.Sprint(counts[mutate.TimedOut])},
		{"  out of memory", fmt.Sprint(counts[mutate.OutOfMemory])},
		{"  unviable", fmt.Sprint(counts[mutate.Unviable])},
		{"  allowed", fmt.Sprint(allowed)},
		{"  survived", fmt.Sprint(len(survivors))},
	})
	for _, m := range survivors {
		fmt.Fprintf(w, "\n%s\n%s", m, m.AllowEntry())
	}
	return len(survivors)
}
