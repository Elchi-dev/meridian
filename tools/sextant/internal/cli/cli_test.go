package cli

import (
	"bytes"
	"errors"
	"runtime"
	"runtime/debug"
	"strings"
	"testing"
)

type result struct {
	code   int
	stdout string
	stderr string
}

func run(args ...string) result {
	var out, errOut bytes.Buffer
	code := Run(args, Env{
		Stdin:   strings.NewReader(""),
		Stdout:  &out,
		Stderr:  &errOut,
		Version: "v9.8.7",
	})
	return result{code: code, stdout: out.String(), stderr: errOut.String()}
}

func TestNoArgumentsPrintsTheOverview(t *testing.T) {
	r := run()
	if r.code != ExitOK {
		t.Fatalf("exit code = %d, want %d", r.code, ExitOK)
	}
	if r.stderr != "" {
		t.Fatalf("stderr = %q, want nothing", r.stderr)
	}
	for _, want := range []string{
		"sextant builds, runs and checks Meridian.",
		"Usage: sextant <command> [arguments]",
		"  help     Show the commands, or details for one command\n",
		"  version  Print the version of sextant\n",
		"Run 'sextant help <command>' for details on one command.",
	} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("overview is missing %q\ngot:\n%s", want, r.stdout)
		}
	}
}

func TestOverviewAlignsEverySummary(t *testing.T) {
	r := run()
	column := -1
	for _, line := range strings.Split(r.stdout, "\n") {
		if !strings.HasPrefix(line, "  ") {
			continue
		}
		name := strings.Fields(line)[0]
		summary := strings.TrimLeft(line[2+len(name):], " ")
		at := len(line) - len(summary)
		if column == -1 {
			column = at
		}
		if at != column {
			t.Errorf("summary of %q starts at column %d, the first one at %d", name, at, column)
		}
	}
	if column == -1 {
		t.Fatal("no command lines found in the overview")
	}
}

func TestHelpFlagsShowTheOverview(t *testing.T) {
	want := run().stdout
	for _, flag := range []string{"-h", "--help", "help"} {
		r := run(flag)
		if r.code != ExitOK || r.stdout != want {
			t.Errorf("%s: code %d, stdout %q; want the overview", flag, r.code, r.stdout)
		}
	}
}

func TestVersionFlagIsTheVersionCommand(t *testing.T) {
	a, b := run("--version"), run("version")
	if a.code != ExitOK || a.stdout != b.stdout {
		t.Fatalf("--version gave %q (code %d), version gave %q", a.stdout, a.code, b.stdout)
	}
}

func TestVersionPrintsTheInjectedVersion(t *testing.T) {
	r := run("version")
	if !strings.HasPrefix(r.stdout, "sextant v9.8.7") || !strings.HasSuffix(r.stdout, "\n") {
		t.Fatalf("stdout = %q", r.stdout)
	}
}

func TestVersionRejectsArguments(t *testing.T) {
	r := run("version", "extra")
	if r.code != ExitUsage {
		t.Fatalf("exit code = %d, want %d", r.code, ExitUsage)
	}
	if r.stderr != "sextant version: version takes no arguments\n" {
		t.Fatalf("stderr = %q", r.stderr)
	}
}

func TestUnknownCommandSuggestsTheClosestOne(t *testing.T) {
	r := run("versoin")
	if r.code != ExitUsage {
		t.Fatalf("exit code = %d, want %d", r.code, ExitUsage)
	}
	want := "sextant: unknown command \"versoin\". Did you mean \"version\"? Run 'sextant help' to see all commands.\n"
	if r.stderr != want {
		t.Fatalf("stderr = %q\nwant     %q", r.stderr, want)
	}
	if r.stdout != "" {
		t.Fatalf("stdout = %q, want nothing", r.stdout)
	}
}

func TestUnknownCommandWithoutCloseMatch(t *testing.T) {
	r := run("flibbertigibbet")
	want := "sextant: unknown command \"flibbertigibbet\". Run 'sextant help' to see all commands.\n"
	if r.code != ExitUsage || r.stderr != want {
		t.Fatalf("code %d, stderr %q", r.code, r.stderr)
	}
}

func TestHelpForOneCommand(t *testing.T) {
	r := run("help", "version")
	if r.code != ExitOK || r.stdout != "sextant version: Print the version of sextant\n" {
		t.Fatalf("code %d, stdout %q", r.code, r.stdout)
	}
}

func TestHelpForUnknownCommandIsAUsageError(t *testing.T) {
	r := run("help", "fly")
	if r.code != ExitUsage || r.stderr != "sextant help: there is no command \"fly\"\n" {
		t.Fatalf("code %d, stderr %q", r.code, r.stderr)
	}
}

func TestHelpWithTwoNamesIsAUsageError(t *testing.T) {
	r := run("help", "help", "version")
	if r.code != ExitUsage || r.stderr != "sextant help: help takes one command name, got 2\n" {
		t.Fatalf("code %d, stderr %q", r.code, r.stderr)
	}
}

func TestExitCodeSeparatesUsageFromFailure(t *testing.T) {
	if got := exitCode(nil); got != ExitOK {
		t.Errorf("nil error: %d", got)
	}
	if got := exitCode(errors.New("disk full")); got != ExitFailure {
		t.Errorf("plain error: %d, want %d", got, ExitFailure)
	}
	if got := exitCode(usageErrorf("bad flag")); got != ExitUsage {
		t.Errorf("usage error: %d, want %d", got, ExitUsage)
	}
	wrapped := errors.Join(errors.New("context"), usageErrorf("bad flag"))
	if got := exitCode(wrapped); got != ExitUsage {
		t.Errorf("wrapped usage error: %d, want %d", got, ExitUsage)
	}
}

func TestVersionLine(t *testing.T) {
	platform := " " + runtime.GOOS + "/" + runtime.GOARCH + " " + runtime.Version()
	if got := versionLine("v1.2.3", "0123456789ab"); got != "sextant v1.2.3 (0123456789ab)"+platform {
		t.Errorf("with revision: %q", got)
	}
	if got := versionLine("dev", ""); got != "sextant dev"+platform {
		t.Errorf("without revision: %q", got)
	}
}

func TestRevisionFromBuildSettings(t *testing.T) {
	tests := []struct {
		name     string
		settings []debug.BuildSetting
		want     string
	}{
		{"nothing recorded", nil, ""},
		{"exactly twelve characters stay", []debug.BuildSetting{{Key: "vcs.revision", Value: "0123456789ab"}}, "0123456789ab"},
		{"longer revisions are cut to twelve", []debug.BuildSetting{{Key: "vcs.revision", Value: "0123456789abc"}}, "0123456789ab"},
		{"modified tree is marked", []debug.BuildSetting{
			{Key: "vcs.revision", Value: "abc"},
			{Key: "vcs.modified", Value: "true"},
		}, "abc+dirty"},
		{"clean tree is not marked", []debug.BuildSetting{
			{Key: "vcs.revision", Value: "abc"},
			{Key: "vcs.modified", Value: "false"},
		}, "abc"},
		{"dirty without revision says nothing", []debug.BuildSetting{{Key: "vcs.modified", Value: "true"}}, ""},
		{"other settings are ignored", []debug.BuildSetting{
			{Key: "GOOS", Value: "linux"},
			{Key: "vcs.revision", Value: "def"},
		}, "def"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := revisionFrom(tt.settings); got != tt.want {
				t.Errorf("revisionFrom = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestEditDistance(t *testing.T) {
	tests := []struct {
		a, b string
		want int
	}{
		{"", "", 0},
		{"abc", "", 3},
		{"", "abc", 3},
		{"help", "help", 0},
		{"abc", "abd", 1},
		{"abc", "abcd", 1},
		{"hepl", "help", 2},
		{"kitten", "sitting", 3},
	}
	for _, tt := range tests {
		if got := editDistance(tt.a, tt.b); got != tt.want {
			t.Errorf("editDistance(%q, %q) = %d, want %d", tt.a, tt.b, got, tt.want)
		}
	}
}

func TestSuggest(t *testing.T) {
	tests := []struct{ in, want string }{
		{"hlep", "help"},
		{"VERSION", "version"},
		{"versi", "version"},
		{"vers", ""},
		{"xyzzy", ""},
	}
	for _, tt := range tests {
		if got := suggest(tt.in); got != tt.want {
			t.Errorf("suggest(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestExitCodesKeepTheirMeaning(t *testing.T) {
	if ExitOK != 0 || ExitFailure != 1 || ExitUsage != 2 {
		t.Fatalf("exit codes changed: ok=%d failure=%d usage=%d; CI scripts depend on 0, 1 and 2", ExitOK, ExitFailure, ExitUsage)
	}
}

func TestCommandTableIsWellFormed(t *testing.T) {
	seen := map[string]bool{}
	for _, c := range commands() {
		if seen[c.name] {
			t.Errorf("command %q is listed twice", c.name)
		}
		seen[c.name] = true
		if c.name != strings.ToLower(c.name) || strings.ContainsAny(c.name, " \t") {
			t.Errorf("command name %q must be one lower-case word", c.name)
		}
		if c.summary == "" {
			t.Errorf("command %q has no summary", c.name)
			continue
		}
		if first := c.summary[:1]; first != strings.ToUpper(first) {
			t.Errorf("summary of %q should start with a capital letter: %q", c.name, c.summary)
		}
		if strings.HasSuffix(c.summary, ".") || strings.HasSuffix(c.summary, " ") {
			t.Errorf("summary of %q should not end in a period or space: %q", c.name, c.summary)
		}
		if c.run == nil {
			t.Errorf("command %q has nothing to run", c.name)
		}
	}
}

func TestEveryCommandHasHelp(t *testing.T) {
	for _, c := range commands() {
		r := run("help", c.name)
		want := "sextant " + c.name + ": " + c.summary + "\n"
		if r.code != ExitOK || r.stdout != want {
			t.Errorf("help %s: code %d, stdout %q, want %q", c.name, r.code, r.stdout, want)
		}
	}
}

func TestHelpFlagTakesACommandName(t *testing.T) {
	a, b := run("-h", "version"), run("help", "version")
	if a.code != ExitOK || a.stdout != b.stdout {
		t.Fatalf("-h version gave %q, help version gave %q", a.stdout, b.stdout)
	}
}

func TestOutputHasNoTrailingWhitespace(t *testing.T) {
	for _, args := range [][]string{nil, {"help", "version"}, {"version"}} {
		r := run(args...)
		for i, line := range strings.Split(r.stdout, "\n") {
			if strings.TrimRight(line, " \t") != line {
				t.Errorf("sextant %v, line %d has trailing whitespace: %q", args, i+1, line)
			}
		}
	}
}

func TestErrorsGoToStderrOnly(t *testing.T) {
	for _, args := range [][]string{{"nope"}, {"help", "nope"}, {"version", "x"}} {
		r := run(args...)
		if r.stdout != "" {
			t.Errorf("sextant %v wrote %q to stdout on failure", args, r.stdout)
		}
		if !strings.HasSuffix(r.stderr, "\n") {
			t.Errorf("sextant %v: stderr %q should end with a newline", args, r.stderr)
		}
	}
}

func TestUsageErrorFormatsItsMessage(t *testing.T) {
	err := usageErrorf("expected %d paths, got %q", 2, "one")
	if err.Error() != `expected 2 paths, got "one"` {
		t.Fatalf("message = %q", err.Error())
	}
}
