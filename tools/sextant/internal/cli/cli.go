// Package cli turns command lines into sextant commands.
package cli

import (
	"errors"
	"fmt"
	"io"
	"runtime"
	"runtime/debug"
	"sort"
	"strings"
)

// Exit codes. Scripts and CI rely on them, so they never change meaning.
const (
	ExitOK      = 0
	ExitFailure = 1
	ExitUsage   = 2
)

// Env is everything a command may touch outside its arguments.
type Env struct {
	Dir     string
	Stdin   io.Reader
	Stdout  io.Writer
	Stderr  io.Writer
	Version string
	// ConfigDir is the user's settings folder, where sextant keeps what
	// belongs to this machine, such as its development key. Empty when
	// the system has none.
	ConfigDir string
}

// UsageError marks a mistake in how sextant was called, as opposed to a
// command that ran and failed. It maps to ExitUsage.
type UsageError struct{ msg string }

func (e *UsageError) Error() string { return e.msg }

func usageErrorf(format string, args ...any) error {
	return &UsageError{msg: fmt.Sprintf(format, args...)}
}

type command struct {
	name    string
	summary string
	run     func(env Env, args []string) error
}

func commands() []command {
	return []command{
		{name: "adr", summary: "Start a new decision record: sextant adr new \"title\"", run: runADR},
		{name: "build", summary: "Build the bootloader", run: runBuild},
		{name: "ci", summary: "Run every check CI runs, in its order", run: runCI},
		{name: "doctor", summary: "Check the tools Meridian needs and install what is missing", run: runDoctor},
		{name: "help", summary: "Show the commands, or details for one command", run: runHelp},
		{name: "image", summary: "Write a bootable disk or ISO image", run: runImage},
		{name: "key", summary: "Make the release key and its certificate: sextant key new release.pem", run: runKey},
		{name: "lint", summary: "Check the project rules that no compiler knows about", run: runLint},
		{name: "mutate", summary: "Change the code on purpose and check the tests notice", run: runMutate},
		{name: "run", summary: "Boot Meridian in QEMU", run: runRun},
		{name: "stats", summary: "Count lines of code and tests per component", run: runStats},
		{name: "test", summary: "Run the tests on this machine and in QEMU", run: runTest},
		{name: "version", summary: "Print the version of sextant", run: runVersion},
	}
}

func lookup(name string) (command, bool) {
	for _, c := range commands() {
		if c.name == name {
			return c, true
		}
	}
	return command{}, false
}

// Run executes one sextant command line and returns the process exit code.
func Run(args []string, env Env) int {
	if len(args) == 0 {
		writeOverview(env.Stdout)
		return ExitOK
	}
	name, rest := args[0], args[1:]
	if name == "-h" || name == "--help" {
		name = "help"
	}
	if name == "--version" {
		name = "version"
	}
	cmd, ok := lookup(name)
	if !ok {
		fmt.Fprintf(env.Stderr, "sextant: unknown command %q.", name)
		if s := suggest(name); s != "" {
			fmt.Fprintf(env.Stderr, " Did you mean %q?", s)
		}
		fmt.Fprintln(env.Stderr, " Run 'sextant help' to see all commands.")
		return ExitUsage
	}
	err := cmd.run(env, rest)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sextant %s: %v\n", cmd.name, err)
	}
	return exitCode(err)
}

func exitCode(err error) int {
	if err == nil {
		return ExitOK
	}
	var usage *UsageError
	if errors.As(err, &usage) {
		return ExitUsage
	}
	return ExitFailure
}

func writeOverview(w io.Writer) {
	fmt.Fprintln(w, "sextant builds, runs and checks Meridian.")
	fmt.Fprintln(w)
	fmt.Fprintln(w, "Usage: sextant <command> [arguments]")
	fmt.Fprintln(w)
	fmt.Fprintln(w, "Commands:")
	cmds := commands()
	width := 0
	for _, c := range cmds {
		width = max(width, len(c.name))
	}
	for _, c := range cmds {
		fmt.Fprintf(w, "  %-*s  %s\n", width, c.name, c.summary)
	}
	fmt.Fprintln(w)
	fmt.Fprintln(w, "Run 'sextant help <command>' for details on one command.")
}

func runHelp(env Env, args []string) error {
	if len(args) == 0 {
		writeOverview(env.Stdout)
		return nil
	}
	if len(args) > 1 {
		return usageErrorf("help takes one command name, got %d", len(args))
	}
	cmd, ok := lookup(args[0])
	if !ok {
		return usageErrorf("there is no command %q", args[0])
	}
	fmt.Fprintf(env.Stdout, "sextant %s: %s\n", cmd.name, cmd.summary)
	return nil
}

func runVersion(env Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("version takes no arguments")
	}
	fmt.Fprintln(env.Stdout, versionLine(env.Version, readRevision()))
	return nil
}

func versionLine(version, revision string) string {
	line := "sextant " + version
	if revision != "" {
		line += " (" + revision + ")"
	}
	return line + " " + runtime.GOOS + "/" + runtime.GOARCH + " " + runtime.Version()
}

// readRevision returns the git commit sextant was built from, shortened, with
// "+dirty" when the tree had uncommitted changes. Go records both for builds
// inside a repository.
func readRevision() string {
	return revisionOf(debug.ReadBuildInfo())
}

func revisionOf(info *debug.BuildInfo, ok bool) string {
	if !ok {
		return ""
	}
	return revisionFrom(info.Settings)
}

func revisionFrom(settings []debug.BuildSetting) string {
	var rev string
	dirty := false
	for _, s := range settings {
		switch s.Key {
		case "vcs.revision":
			rev = s.Value
		case "vcs.modified":
			dirty = s.Value == "true"
		}
	}
	rev = rev[:min(len(rev), 12)]
	if rev != "" && dirty {
		rev += "+dirty"
	}
	return rev
}

// suggest returns the command closest to a mistyped name, or "" when nothing
// is close enough to be a likely typo.
func suggest(name string) string {
	best, bestDist := "", 3
	names := make([]string, 0, len(commands()))
	for _, c := range commands() {
		names = append(names, c.name)
	}
	sort.Strings(names)
	for _, n := range names {
		if d := editDistance(strings.ToLower(name), n); d < bestDist {
			best, bestDist = n, d
		}
	}
	return best
}

func editDistance(a, b string) int {
	prev := make([]int, len(b)+1)
	cur := make([]int, len(b)+1)
	for j := range prev {
		prev[j] = j
	}
	for i := 1; i <= len(a); i++ {
		cur[0] = i
		for j := 1; j <= len(b); j++ {
			cost := 1
			if a[i-1] == b[j-1] {
				cost = 0
			}
			cur[j] = min(prev[j]+1, cur[j-1]+1, prev[j-1]+cost)
		}
		prev, cur = cur, prev
	}
	return prev[len(b)]
}
