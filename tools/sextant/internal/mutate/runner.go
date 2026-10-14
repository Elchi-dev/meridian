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
	// Done, when set, is called after every mutant from the worker that
	// ran it, so it must be safe for concurrent use.
	Done func(Result)
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
		w := &worker{
			dir:     filepath.Join(scratch, fmt.Sprintf("tree-%d", i)),
			target:  filepath.Join(scratch, fmt.Sprintf("target-%d", i)),
			tmp:     filepath.Join(scratch, fmt.Sprintf("tmp-%d", i)),
			maxFile: maxFile,
		}
		if err := copyTree(r.Root, w.dir); err != nil {
			return nil, err
		}
		if err := os.MkdirAll(w.tmp, 0o755); err != nil {
			return nil, err
		}
		workers[i] = w
	}

	timeouts := map[string]time.Duration{}
	for key, m := range first {
		started := time.Now()
		run := workers[0].test(ctx, m.Path, targets[m.Path], 0)
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if run.err != nil {
			return nil, fmt.Errorf("the tests of %s fail before any mutation, fix them first:\n%s", key, tail(run.output, 20))
		}
		timeouts[key] = r.timeoutFor(time.Since(started))
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
				outcome, err := w.try(ctx, m, originals[m.Path], t, timeouts[testKey(m.Path, t)])
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

// timeoutFor is the limit for one mutant's test run, given how long the
// same tests took without any mutant.
func (r *Runner) timeoutFor(baseline time.Duration) time.Duration {
	if r.Timeout > 0 {
		return r.Timeout
	}
	return 3*baseline + 30*time.Second
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

type testRun struct {
	err      error
	output   []byte
	timedOut bool
}

// try writes the mutant into the worker's copy, tests it, and writes the
// original back. Writing, rather than moving a saved copy into place, gives
// the file a fresh modification time: Cargo decides what to rebuild by
// those times and would otherwise keep testing the mutant's binary.
func (w *worker) try(ctx context.Context, m Mutant, original []byte, t Target, timeout time.Duration) (Outcome, error) {
	file := filepath.Join(w.dir, filepath.FromSlash(m.Path))
	if err := os.WriteFile(file, m.Apply(original), 0o644); err != nil {
		return 0, err
	}
	run := w.test(ctx, m.Path, t, timeout)
	if err := os.WriteFile(file, original, 0o644); err != nil {
		return 0, err
	}
	if ctx.Err() != nil {
		return 0, ctx.Err()
	}
	return classify(t.Kind, run), nil
}

func classify(kind string, run testRun) Outcome {
	switch {
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

// test runs the tests that cover file. A zero timeout means none.
func (w *worker) test(ctx context.Context, file string, t Target, timeout time.Duration) testRun {
	if timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
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
	if nice, err := exec.LookPath("nice"); err == nil && runtime.GOOS != "windows" {
		args = append([]string{"-n", "15", name}, args...)
		name = nice
	}
	name, args = limitFiles(name, args, w.maxFile)

	// Every run gets a temporary folder of its own, removed right after.
	// A test killed by the timeout never cleans up after itself, so this
	// is the only cleanup that always happens.
	tmp, err := os.MkdirTemp(w.tmp, "run-")
	if err != nil {
		return testRun{err: err}
	}
	defer os.RemoveAll(tmp)

	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "CARGO_TARGET_DIR="+w.target, "TMPDIR="+tmp, "TMP="+tmp, "TEMP="+tmp)
	cmd.WaitDelay = 5 * time.Second
	killTree(cmd)
	out, err := cmd.CombinedOutput()
	reapTree(cmd)
	return testRun{err: err, output: out, timedOut: errors.Is(ctx.Err(), context.DeadlineExceeded)}
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
