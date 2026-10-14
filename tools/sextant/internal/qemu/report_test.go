package qemu

import (
	"strings"
	"testing"
)

// What OVMF actually wrote at the start of a boot, copied from a real run.
const ovmfStart = "\x1b[2J\x1b[01;01H\x1b[=3h\x1b[2J\x1b[01;01H\x1b[2J\x1b[01;01H\x1b[8;056;240t\x1b[2J\x1b[01;01H" +
	"BdsDxe: loading Boot0002 \"UEFI QEMU HARDDISK QM00001 \" from PciRoot(0x0)/Pci(0x1F,0x2)/Sata(0x0,0xFFFF,0x0)\r\n"

func TestCleanRemovesWhatOVMFAdds(t *testing.T) {
	got := Clean(ovmfStart)
	want := "BdsDxe: loading Boot0002 \"UEFI QEMU HARDDISK QM00001 \" from PciRoot(0x0)/Pci(0x1F,0x2)/Sata(0x0,0xFFFF,0x0)\n"
	if got != want {
		t.Fatalf("got %q", got)
	}
}

func TestCleanCases(t *testing.T) {
	tests := []struct{ in, want string }{
		{"", ""},
		{"plain\n", "plain\n"},
		{"a\r\nb\r\n", "a\nb\n"},
		{"\x1b[0m\x1b[1;37;40mbright\x1b[0m", "bright"},
		{"\x1b[?25lhidden cursor\x1b[?25h", "hidden cursor"},
		{"x\x1b(By", "xBy"},
		{"x\x1bcy", "xy"},
		{"end\x1b[", "end"},
		{"end\x1b[12;3", "end"},
		{"end\x1b", "end"},
		{"[not an escape]", "[not an escape]"},
		{"\x1b[01;01H@\x1b[2J~", "@~"},
		{"\x1b[1\x80\xffmx", "x"},
	}
	for _, tt := range tests {
		if got := Clean(tt.in); got != tt.want {
			t.Errorf("Clean(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestParseReport(t *testing.T) {
	r := ParseReport("firmware noise\nTEST uefi.vendor ok\nTEST uefi.stall FAILED Stall(1000) failed\n" +
		"console probe: TEST not at the start\nTEST boot.uefi_version ok\nTEST odd.result maybe\nTESTS DONE 1 failed\n")
	if strings.Join(r.Passed, ",") != "uefi.vendor,boot.uefi_version" {
		t.Errorf("passed = %v", r.Passed)
	}
	if len(r.Failed) != 1 || r.Failed[0] != (Failure{Name: "uefi.stall", Why: "Stall(1000) failed"}) {
		t.Errorf("failed = %+v", r.Failed)
	}
	if !r.Done {
		t.Error("TESTS DONE was there")
	}
}

func TestParseReportEdges(t *testing.T) {
	r := ParseReport("TEST a FAILED\nTEST b\nTESTS DONE\nTESTS DONE 0 failed")
	if len(r.Failed) != 1 || r.Failed[0].Why != "" || r.Failed[0].Name != "a" {
		t.Errorf("a failure without a reason: %+v", r.Failed)
	}
	if len(r.Passed) != 0 {
		t.Errorf("a line without a result is not a pass: %v", r.Passed)
	}
	if !r.Done {
		t.Error("the second DONE line counts")
	}
	if r := ParseReport(""); r.Done || r.Passed != nil || r.Failed != nil {
		t.Errorf("empty output, empty report: %+v", r)
	}
}

func TestVerdict(t *testing.T) {
	pass := Report{Passed: []string{"a", "b"}, Done: true}
	fail := Report{Passed: []string{"a"}, Failed: []Failure{{"b", "broke"}, {"c", "too"}}, Done: true}
	tests := []struct {
		name     string
		r        Report
		code     int
		timedOut bool
		want     string
	}{
		{"pass", pass, ExitPass, false, ""},
		{"silent timeout", Report{}, 0, true, "the image never reported a test before the timeout; the last line was \"last\". Is it built with the qemu-test feature, and does the firmware find it?"},
		{"stalled", Report{Passed: []string{"a"}}, 0, true, "the image stopped reporting after 1 checks and hit the timeout; the last line was \"last\""},
		{"failures", fail, ExitFail, false, "2 of 3 checks failed: b (broke), c (too)"},
		{"no done line", Report{Passed: []string{"a"}}, 1, false, "the image ended without its TESTS DONE line (exit code 1); the last line was \"last\""},
		{"wrong exit", pass, ExitFail, false, "all checks passed but QEMU exited with 35 instead of 33"},
		{"failures win over exit code", fail, ExitPass, false, "2 of 3 checks failed: b (broke), c (too)"},
	}
	for _, tt := range tests {
		err := tt.r.Verdict(tt.code, tt.timedOut, "last")
		got := ""
		if err != nil {
			got = err.Error()
		}
		if got != tt.want {
			t.Errorf("%s:\ngot  %q\nwant %q", tt.name, got, tt.want)
		}
	}
}

func TestLastLine(t *testing.T) {
	tests := []struct{ in, want string }{
		{"a\nb\n", "b"},
		{"a\nb  \n\n\n", "b"},
		{"  only  ", "only"},
		{"", ""},
		{"\n\n", ""},
	}
	for _, tt := range tests {
		if got := LastLine(tt.in); got != tt.want {
			t.Errorf("LastLine(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}
