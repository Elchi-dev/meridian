package qemu

import (
	"fmt"
	"strings"
)

// Failure is one check a test image reported as failed.
type Failure struct {
	Name string
	Why  string
}

// Report is what a test image said over the serial port, in the protocol
// of docs/testing.md: "TEST <name> ok", "TEST <name> FAILED <why>" and
// "TESTS DONE <n> failed".
type Report struct {
	Passed []string
	Failed []Failure
	Done   bool
}

// Clean removes what the firmware's terminal emulation adds to the serial
// stream: escape sequences for colours and cursor movement, and carriage
// returns.
func Clean(raw string) string {
	var b strings.Builder
	for i := 0; i < len(raw); i++ {
		c := raw[i]
		switch {
		case c == '\r':
		case c == 0x1b && i+1 < len(raw) && raw[i+1] == '[':
			i += 2
			for i < len(raw) && (raw[i] < 0x40 || raw[i] > 0x7e) {
				i++
			}
		case c == 0x1b:
			i++
		default:
			b.WriteByte(c)
		}
	}
	return b.String()
}

// ParseReport reads the test lines out of cleaned serial output. Text
// around them, such as firmware messages, is ignored.
func ParseReport(clean string) Report {
	var r Report
	for _, line := range strings.Split(clean, "\n") {
		rest, ok := strings.CutPrefix(line, "TEST ")
		if !ok {
			if strings.HasPrefix(line, "TESTS DONE ") {
				r.Done = true
			}
			continue
		}
		name, result, _ := strings.Cut(rest, " ")
		switch {
		case result == "ok":
			r.Passed = append(r.Passed, name)
		case strings.HasPrefix(result, "FAILED"):
			r.Failed = append(r.Failed, Failure{Name: name, Why: strings.TrimSpace(strings.TrimPrefix(result, "FAILED"))})
		}
	}
	return r
}

// Verdict says whether a test boot passed, and if not, what to look at.
// A pass needs the pass exit code, a finished report and no failures.
func (r Report) Verdict(exitCode int, timedOut bool, lastLine string) error {
	switch {
	case timedOut && len(r.Passed)+len(r.Failed) == 0:
		return fmt.Errorf("the image never reported a test before the timeout; the last line was %q. Is it built with the qemu-test feature, and does the firmware find it?", lastLine)
	case timedOut:
		return fmt.Errorf("the image stopped reporting after %d checks and hit the timeout; the last line was %q", len(r.Passed)+len(r.Failed), lastLine)
	case len(r.Failed) > 0:
		var names []string
		for _, f := range r.Failed {
			names = append(names, f.Name+" ("+f.Why+")")
		}
		return fmt.Errorf("%d of %d checks failed: %s", len(r.Failed), len(r.Passed)+len(r.Failed), strings.Join(names, ", "))
	case !r.Done:
		return fmt.Errorf("the image ended without its TESTS DONE line (exit code %d); the last line was %q", exitCode, lastLine)
	case exitCode != ExitPass:
		return fmt.Errorf("all checks passed but QEMU exited with %d instead of %d", exitCode, ExitPass)
	}
	return nil
}

// LastLine is the last non-empty line of cleaned output, for messages.
func LastLine(clean string) string {
	lines := strings.Split(strings.TrimRight(clean, "\n "), "\n")
	return strings.TrimSpace(lines[len(lines)-1])
}
