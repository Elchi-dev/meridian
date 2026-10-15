package cli

import (
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"go/format"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// ciStep is one check of 'sextant ci', run as a process of its own.
type ciStep struct {
	name string
	// dir is where it runs, from the root of the repository.
	dir string
	// args is the program and its arguments; "sextant" is this sextant.
	args []string
	// check runs inside sextant instead, on the step's folder.
	check func(dir string, out io.Writer) error
}

// ciSteps are the checks CI runs, in its order. The workflow runs
// 'sextant ci', so this list is the only place they are written down.
func ciSteps() []ciStep {
	clippy := func(name string, args ...string) ciStep {
		args = append([]string{"cargo", "clippy", "--locked"}, args...)
		return ciStep{name: name, dir: ".", args: append(args, "--", "-D", "warnings")}
	}
	return []ciStep{
		{name: "project rules", dir: ".", args: []string{"sextant", "lint"}},
		{name: "test lines", dir: ".", args: []string{"sextant", "stats", "--check"}},
		{name: "Go formatting", dir: "tools/sextant", check: goFormatted},
		{name: "Rust formatting", dir: ".", args: []string{"cargo", "fmt", "--all", "--", "--check"}},
		clippy("clippy", "--workspace", "--all-targets"),
		clippy("clippy for the loader", "-p", "boot", "--target", "x86_64-unknown-uefi"),
		clippy("clippy for the loader's test image", "-p", "boot", "--target", "x86_64-unknown-uefi", "--features", "qemu-test"),
		clippy("clippy for the kernel", "-p", "kernel", "--target", "x86_64-unknown-none"),
		clippy("clippy for the kernel's test image", "-p", "kernel", "--target", "x86_64-unknown-none", "--features", "qemu-test"),
		{name: "go vet", dir: "tools/sextant", args: []string{"go", "vet", "./..."}},
		{name: "Go tests with the race detector", dir: "tools/sextant", args: []string{"go", "test", "-race", "-count=1", "./..."}},
		{name: "Rust tests", dir: ".", args: []string{"cargo", "test", "--locked", "--workspace"}},
		{name: "the QEMU test image", dir: ".", args: []string{"sextant", "test", "--qemu"}},
	}
}

func runCI(env Env, args []string) error {
	return ciCommand(env, args, ciSteps())
}

// ciCommand is 'sextant ci' with steps to run.
func ciCommand(env Env, args []string, steps []ciStep) error {
	flags := flag.NewFlagSet("ci", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	list := flags.Bool("list", false, "only list the checks")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	if *list {
		for _, s := range steps {
			what := strings.Join(s.args, " ")
			if s.check != nil {
				what = "checked by sextant itself"
			}
			fmt.Fprintf(env.Stdout, "%-36s %s\n", s.name, what)
		}
		return nil
	}
	if os.Getenv(insideChecks) != "" {
		return errors.New("sextant ci is running inside its own checks, which would start them again without end")
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	self, err := os.Executable()
	if err != nil {
		return fmt.Errorf("find sextant itself: %w", err)
	}
	r := ciRun{
		root:   s.root,
		self:   self,
		env:    []string{"CARGO_TARGET_DIR=" + s.settings.BuildDir, insideChecks + "=1"},
		out:    env.Stdout,
		errOut: env.Stderr,
		github: os.Getenv("GITHUB_ACTIONS") == "true",
	}
	return r.all(context.Background(), steps)
}

// ciRun runs steps one after the other and stops at the first that fails.
type ciRun struct {
	root, self  string
	env         []string
	out, errOut io.Writer
	// github folds each step's output into a group in the GitHub Actions
	// log.
	github bool
}

func (r ciRun) all(ctx context.Context, steps []ciStep) error {
	start := time.Now()
	for i, s := range steps {
		if r.github {
			fmt.Fprintf(r.out, "::group::%s\n", s.name)
		} else {
			fmt.Fprintf(r.out, "== %d/%d %s\n", i+1, len(steps), s.name)
		}
		began := time.Now()
		err := r.one(ctx, s)
		if r.github {
			fmt.Fprintln(r.out, "::endgroup::")
		}
		if err != nil {
			return fmt.Errorf("%s failed: %w", s.name, err)
		}
		fmt.Fprintf(r.out, "ok %s in %s\n", s.name, time.Since(began).Round(time.Second))
	}
	fmt.Fprintf(r.out, "all %d checks passed in %s\n", len(steps), time.Since(start).Round(time.Second))
	return nil
}

func (r ciRun) one(ctx context.Context, s ciStep) error {
	dir := filepath.Join(r.root, filepath.FromSlash(s.dir))
	if s.check != nil {
		return s.check(dir, r.out)
	}
	program := s.args[0]
	if program == "sextant" {
		program = r.self
	}
	cmd := exec.CommandContext(ctx, program, s.args[1:]...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), r.env...)
	cmd.Stdout, cmd.Stderr = r.out, r.errOut
	return cmd.Run()
}

// goFormatted lists the Go files under dir that gofmt would change, and
// fails if there are any. It formats with go/format, which is gofmt's own
// code in the Go that built sextant, so an older gofmt on the PATH does
// not decide.
func goFormatted(dir string, out io.Writer) error {
	var messy []string
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() || filepath.Ext(path) != ".go" {
			return err
		}
		src, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		formatted, err := format.Source(src)
		if err != nil {
			return fmt.Errorf("%s: %w", path, err)
		}
		if !bytes.Equal(src, formatted) {
			rel, _ := filepath.Rel(dir, path)
			messy = append(messy, filepath.ToSlash(rel))
		}
		return nil
	})
	if err != nil {
		return err
	}
	for _, f := range messy {
		fmt.Fprintln(out, f)
	}
	if len(messy) > 0 {
		return fmt.Errorf("gofmt would change %d %s", len(messy), plural(len(messy), "file", "files"))
	}
	return nil
}
