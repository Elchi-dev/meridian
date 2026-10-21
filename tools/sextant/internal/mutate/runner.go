package mutate

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"time"
)

type Outcome int

const (
	Killed Outcome = iota + 1
	Survived
	Unviable
	TimedOut
	// OutOfMemory is a mutant whose tests were stopped for holding more
	// memory than the runner allows, which counts as killed, like a
	// timeout.
	OutOfMemory
)

func (o Outcome) String() string {
	switch o {
	case Killed:
		return "killed"
	case Survived:
		return "survived"
	case Unviable:
		return "unviable"
	case TimedOut:
		return "timed out"
	case OutOfMemory:
		return "out of memory"
	}
	return fmt.Sprintf("Outcome(%d)", int(o))
}

type Result struct {
	Mutant  Mutant
	Outcome Outcome
}

// Target says how to test a mutated file: its component and whether that
// is a Rust crate or a Go module.
type Target struct {
	Component string
	Kind      string
	// Machine says the component has checks that run in a machine, the
	// QEMU test image, for code the host tests cannot reach.
	Machine bool
}

// Runner applies mutants in private copies of a repository and runs the
// tests of the component each mutant belongs to.
type Runner struct {
	Root string
	Jobs int
	// Timeout for one test run. Zero means three times the run without
	// any mutant, plus 30 seconds.
	Timeout time.Duration
	// Scratch is where the worker copies, their Cargo builds and every
	// test run's temporary folder go; the system's temporary folder when
	// empty. It belongs on a disk, not in a RAM-backed /tmp: a mutant that
	// loops while writing an image fills whatever is there.
	Scratch string
	// MaxFileSize caps every file a test run writes, 1 GiB when zero.
	MaxFileSize int64
	// MaxMemory caps the memory one test run holds, all of its processes
	// together, where the system shows that, which is Linux. Zero means
	// three times the most the run without any mutant held, and never
	// less than 1 GiB.
	MaxMemory int64
	// Done, when set, is called after every mutant from the worker that
	// ran it, so it must be safe for concurrent use.
	Done func(Result)
	// Machine is the command that runs the machine checks in a worker's
	// copy, from its root, for a Rust mutant whose component has them and
	// whose host tests passed. Empty leaves them out. Each copy gets a
	// sextant.local.meri that builds into the worker's own folder.
	Machine []string
}

// machineKey is the timeout key of the machine checks, which are the same
// for every component.
const machineKey = "machine"

// localSettings is the sextant.local.meri of a worker's copy: its builds go
// to the worker's own folder. Forward slashes, since a backslash starts an
// escape in a .meri string.
func localSettings(target string) []byte {
	return []byte("build {\n    dir = \"" + filepath.ToSlash(target) + "\"\n}\n")
}

// skipCopy are the folders a worker copy leaves out: history and build
// output, which the tests do not need and which can be large.
var skipCopy = map[string]bool{".git": true, "target": true, "out": true, "node_modules": true}

// Run tests every mutant and returns the results in the order given.
// targets maps each mutant's path to its component.
func (r *Runner) Run(ctx context.Context, mutants []Mutant, targets map[string]Target) ([]Result, error) {
	if len(mutants) == 0 {
		return nil, nil
	}
	originals := map[string][]byte{}
	first := map[string]Mutant{}
	for _, m := range mutants {
		t, ok := targets[m.Path]
		if !ok {
			return nil, fmt.Errorf("%s belongs to no component", m.Path)
		}
		originals[m.Path] = nil
		first[testKey(m.Path, t)] = m
	}
	for p := range originals {
		src, err := os.ReadFile(filepath.Join(r.Root, filepath.FromSlash(p)))
		if err != nil {
			return nil, err
		}
		originals[p] = src
	}

	jobs := max(r.Jobs, 1)
	scratch, err := os.MkdirTemp(r.Scratch, "sextant-mutate-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(scratch)
	maxFile := r.MaxFileSize
	if maxFile <= 0 {
		maxFile = 1 << 30
	}
	workers := make([]*worker, jobs)
	for i := range workers {
		if workers[i], err = r.newWorker(scratch, i, maxFile); err != nil {
			return nil, err
		}
	}

	limits := map[string]limit{}
	for key, m := range first {
		started := time.Now()
		run := workers[0].test(ctx, m.Path, targets[m.Path], limit{})
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if run.err != nil {
			return nil, fmt.Errorf("the tests of %s fail before any mutation, fix them first:\n%s", key, tail(run.output, 20))
		}
		limits[key] = limit{r.timeoutFor(time.Since(started)), r.memoryFor(run.peak)}
	}
	if len(r.Machine) > 0 && slices.ContainsFunc(mutants, func(m Mutant) bool { return targets[m.Path].Machine }) {
		started := time.Now()
		run := workers[0].run(ctx, workers[0].dir, r.Machine, limit{})
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if run.err != nil {
			return nil, fmt.Errorf("the machine checks fail before any mutation, fix them first:\n%s", tail(run.output, 20))
		}
		limits[machineKey] = limit{r.timeoutFor(time.Since(started)), r.memoryFor(run.peak)}
	}

	results := make([]Result, len(mutants))
	next := make(chan int)
	var wg sync.WaitGroup
	var firstErr error
	var mu sync.Mutex
	for _, w := range workers {
		wg.Add(1)
		go func(w *worker) {
			defer wg.Done()
			for i := range next {
				m := mutants[i]
				t := targets[m.Path]
				outcome, err := w.try(ctx, m, originals[m.Path], t, limits[testKey(m.Path, t)], r.Machine, limits[machineKey])
				if err != nil {
					mu.Lock()
					firstErr = firstOf(firstErr, err)
					mu.Unlock()
					continue
				}
				results[i] = Result{Mutant: m, Outcome: outcome}
				if r.Done != nil {
					r.Done(results[i])
				}
			}
		}(w)
	}
dispatch:
	for i := range mutants {
		select {
		case next <- i:
		case <-ctx.Done():
			break dispatch
		}
	}
	close(next)
	wg.Wait()
	if firstErr != nil {
		return nil, firstErr
	}
	return results, ctx.Err()
}

// newWorker sets up worker i in scratch: its copy of the repository, its
// build folder and its temporary folder. With machine checks, the copy's
// settings send sextant's builds to the worker's build folder as well.
func (r *Runner) newWorker(scratch string, i int, maxFile int64) (*worker, error) {
	w := &worker{
		dir:     filepath.Join(scratch, fmt.Sprintf("tree-%d", i)),
		target:  filepath.Join(scratch, fmt.Sprintf("target-%d", i)),
		tmp:     filepath.Join(scratch, fmt.Sprintf("tmp-%d", i)),
		maxFile: maxFile,
	}
	if err := copyTree(r.Root, w.dir); err != nil {
		return nil, err
	}
	if len(r.Machine) > 0 {
		if err := os.WriteFile(filepath.Join(w.dir, "sextant.local.meri"), localSettings(w.target), 0o644); err != nil {
			return nil, err
		}
	}
	return w, os.MkdirAll(w.tmp, 0o755)
}

// timeoutFor is the limit for one mutant's test run, given how long the
// same tests took without any mutant.
func (r *Runner) timeoutFor(baseline time.Duration) time.Duration {
	if r.Timeout > 0 {
		return r.Timeout
	}
	return 3*baseline + 30*time.Second
}

// memoryFor is the most memory one mutant's test run may hold, given the
// most the same tests held without any mutant.
func (r *Runner) memoryFor(baseline int64) int64 {
	if r.MaxMemory > 0 {
		return r.MaxMemory
	}
	return max(3*baseline, 1<<30)
}

func firstOf(first, err error) error {
	if first != nil {
		return first
	}
	return err
}

type worker struct {
	dir     string
	target  string
	tmp     string
	maxFile int64
}

// limit bounds one test run: how long it may take and how much memory its
// processes may hold together. Zero means no bound.
type limit struct {
	time   time.Duration
	memory int64
}

type testRun struct {
	err      error
	output   []byte
	timedOut bool
	// overMemory says the run was stopped for holding more than its limit.
	overMemory bool
	// peak is the most memory the run was seen holding.
	peak int64
}

// try writes the mutant into the worker's copy, tests it, and writes the
// original back. Writing, rather than moving a saved copy into place, gives
// the file a fresh modification time: Cargo decides what to rebuild by
// those times and would otherwise keep testing the mutant's binary.
//
// A mutant that survives the host tests of a component with machine checks
// meets those too, when the runner has a command for them.
func (w *worker) try(ctx context.Context, m Mutant, original []byte, t Target, bound limit, machine []string, machineBound limit) (Outcome, error) {
	file := filepath.Join(w.dir, filepath.FromSlash(m.Path))
	if err := os.WriteFile(file, m.Apply(original), 0o644); err != nil {
		return 0, err
	}
	outcome := classify(t.Kind, w.test(ctx, m.Path, t, bound))
	if outcome == Survived && t.Machine && len(machine) > 0 {
		outcome = classify("machine", w.run(ctx, w.dir, machine, machineBound))
	}
	if err := os.WriteFile(file, original, 0o644); err != nil {
		return 0, err
	}
	if ctx.Err() != nil {
		return 0, ctx.Err()
	}
	return outcome, nil
}

func classify(kind string, run testRun) Outcome {
	switch {
	case run.overMemory:
		return OutOfMemory
	case run.timedOut || bytes.Contains(run.output, []byte("panic: test timed out")):
		return TimedOut
	case run.err == nil:
		return Survived
	case kind == "go" && (bytes.Contains(run.output, []byte("[build failed]")) || bytes.Contains(run.output, []byte("[setup failed]"))):
		return Unviable
	case kind == "rust" && bytes.Contains(run.output, []byte("error: could not compile")):
		return Unviable
	}
	return Killed
}

// testKey names the test command a file needs: one per Go package, one per
// Rust crate.
func testKey(file string, t Target) string {
	if t.Kind == "go" {
		return "go:" + path.Dir(file)
	}
	return t.Kind + ":" + t.Component
}

// lowPriority runs a test command under nice where there is one, so a long
// mutation run leaves the machine usable.
func lowPriority(name string, args []string) (string, []string) {
	nice, err := exec.LookPath("nice")
	if err != nil || runtime.GOOS == "windows" {
		return name, args
	}
	return nice, append([]string{"-n", "15", name}, args...)
}

// test runs the tests that cover file within bound.
func (w *worker) test(ctx context.Context, file string, t Target, bound limit) testRun {
	var name string
	var args []string
	dir := filepath.Join(w.dir, filepath.FromSlash(t.Component))
	switch t.Kind {
	case "go":
		rel, err := filepath.Rel(dir, filepath.Join(w.dir, filepath.FromSlash(path.Dir(file))))
		if err != nil {
			return testRun{err: err}
		}
		// One failure is enough to kill a mutant; the tests after it only
		// cost time.
		name, args = "go", []string{"test", "-failfast", "./" + filepath.ToSlash(rel)}
	case "rust":
		name, args = "cargo", []string{"test", "-q", "--manifest-path", filepath.Join(dir, "Cargo.toml")}
		dir = w.dir
	default:
		return testRun{err: fmt.Errorf("%s: no tests for components of kind %q", t.Component, t.Kind)}
	}
	return w.run(ctx, dir, append([]string{name}, args...), bound)
}

// run runs command in dir with the worker's build folder and a temporary
// folder of its own, within bound.
func (w *worker) run(ctx context.Context, dir string, command []string, bound limit) testRun {
	if bound.time > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, bound.time)
		defer cancel()
	}
	name, args := lowPriority(command[0], command[1:])
	name, args = limitFiles(name, args, w.maxFile)

	// Every run gets a temporary folder of its own, removed right after.
	// A test killed by the timeout never cleans up after itself, so this
	// is the only cleanup that always happens.
	tmp, err := os.MkdirTemp(w.tmp, "run-")
	if err != nil {
		return testRun{err: err}
	}
	defer os.RemoveAll(tmp)

	stopped, stop := context.WithCancel(ctx)
	defer stop()
	cmd := exec.CommandContext(stopped, name, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "CARGO_TARGET_DIR="+w.target, "TMPDIR="+tmp, "TMP="+tmp, "TEMP="+tmp)
	cmd.WaitDelay = 5 * time.Second
	killTree(cmd)
	var out bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &out
	if err := cmd.Start(); err != nil {
		return testRun{err: err}
	}
	watched := make(chan memoryWatch)
	ended := make(chan struct{})
	go func() { watched <- watchMemory(tmp, bound.memory, ended, stop) }()
	err = cmd.Wait()
	close(ended)
	seen := <-watched
	reapTree(cmd)
	sweep(tmp)
	return testRun{err: err, output: out.Bytes(), timedOut: errors.Is(ctx.Err(), context.DeadlineExceeded), overMemory: seen.over, peak: seen.peak}
}

// over says a run that holds now bytes holds more than most, where a most
// of zero is no bound.
func over(now, most int64) bool {
	return most > 0 && now > most
}

// memoryLook is how often a run's memory is looked at.
const memoryLook = 200 * time.Millisecond

// memoryWatch is what watchMemory saw of a run.
type memoryWatch struct {
	peak int64
	over bool
}

// watchMemory looks at the memory the run in tmp holds until ended closes,
// and calls stop once that is more than most bytes, which zero leaves
// unbounded. A mutant that keeps allocating would otherwise take the
// memory of the whole machine before any timeout comes.
func watchMemory(tmp string, most int64, ended <-chan struct{}, stop func()) memoryWatch {
	var seen memoryWatch
	tick := time.NewTicker(memoryLook)
	defer tick.Stop()
	for {
		select {
		case <-ended:
			return seen
		case <-tick.C:
		}
		now := held(tmp)
		seen.peak = max(seen.peak, now)
		if over(now, most) {
			seen.over = true
			stop()
			sweep(tmp)
			return seen
		}
	}
}

// copyTree copies the sources of a repository, leaving out skipCopy and
// the destination itself when it lies inside the repository.
func copyTree(from, to string) error {
	from, err := filepath.Abs(from)
	if err != nil {
		return err
	}
	if to, err = filepath.Abs(to); err != nil {
		return err
	}
	return filepath.WalkDir(from, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() && p == to {
			return fs.SkipDir
		}
		rel, err := filepath.Rel(from, p)
		if err != nil {
			return err
		}
		dest := filepath.Join(to, rel)
		switch {
		case d.IsDir() && skipCopy[d.Name()] && rel != ".":
			return fs.SkipDir
		case d.IsDir():
			return os.MkdirAll(dest, 0o755)
		case d.Type()&fs.ModeSymlink != 0:
			link, err := os.Readlink(p)
			if err != nil {
				return err
			}
			return os.Symlink(link, dest)
		}
		info, err := d.Info()
		if err != nil {
			return err
		}
		data, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		return os.WriteFile(dest, data, info.Mode().Perm())
	})
}

func tail(out []byte, lines int) string {
	parts := strings.Split(strings.TrimRight(string(out), "\n"), "\n")
	return strings.Join(parts[max(len(parts)-lines, 0):], "\n")
}
