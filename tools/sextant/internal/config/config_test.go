package config

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func repo(t *testing.T, main string, local ...string) string {
	t.Helper()
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, File), []byte(main), 0o644); err != nil {
		t.Fatal(err)
	}
	if len(local) > 0 {
		if err := os.WriteFile(filepath.Join(root, LocalFile), []byte(local[0]), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func load(t *testing.T, main string, local ...string) Config {
	t.Helper()
	c, err := Load(repo(t, main, local...))
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	return c
}

func loadErr(t *testing.T, main string, local ...string) string {
	t.Helper()
	_, err := Load(repo(t, main, local...))
	if err == nil {
		t.Fatal("Load should fail")
	}
	return err.Error()
}

func TestEmptyFileGivesTheDefaults(t *testing.T) {
	root := repo(t, "# nothing set\n")
	c, err := Load(root)
	if err != nil {
		t.Fatal(err)
	}
	want := Defaults(root)
	if c != want {
		t.Fatalf("got %+v\nwant %+v", c, want)
	}
	if want.MinRatio != 1.5 || want.QEMU.Memory != 512<<20 || want.QEMU.CPUs != 4 || want.QEMU.Accel != "auto" || want.QEMU.Timeout != 2*time.Minute {
		t.Fatalf("defaults changed: %+v", want)
	}
	if want.BuildDir != filepath.Join(root, "target") || want.Root != root {
		t.Fatalf("build dir %q, root %q", want.BuildDir, want.Root)
	}
}

func TestEverySetting(t *testing.T) {
	c := load(t, `
stats {
    min-ratio = 2.5
}
build {
    dir = "/tmp/meridian-build"
}
qemu {
    memory = 2GiB
    cpus = 12
    accel = tcg
    firmware = "/fw/code.fd"
    firmware-vars = "/fw/vars.fd"
    timeout = 90s
}
`)
	if c.MinRatio != 2.5 || c.BuildDir != "/tmp/meridian-build" {
		t.Errorf("stats or build: %+v", c)
	}
	want := QEMU{Memory: 2 << 30, CPUs: 12, Accel: "tcg", Code: "/fw/code.fd", Vars: "/fw/vars.fd", Timeout: 90 * time.Second}
	if c.QEMU != want {
		t.Errorf("qemu = %+v\nwant   %+v", c.QEMU, want)
	}
}

func TestRelativeBuildDirIsUnderTheRoot(t *testing.T) {
	root := repo(t, "build { dir = \"out/cargo\" }\n")
	c, err := Load(root)
	if err != nil {
		t.Fatal(err)
	}
	if c.BuildDir != filepath.Join(root, "out", "cargo") {
		t.Fatalf("build dir = %q", c.BuildDir)
	}
}

func TestLocalFileWins(t *testing.T) {
	c := load(t, "qemu {\n    cpus = 2\n    memory = 1GiB\n}\nbuild { dir = \"a\" }\n", "qemu { cpus = 8 }\nbuild { dir = \"/fast/tmpfs\" }\n")
	if c.QEMU.CPUs != 8 || c.QEMU.Memory != 1<<30 || c.BuildDir != "/fast/tmpfs" {
		t.Fatalf("got %+v", c)
	}
}

func TestTheLastBlockOfANameCounts(t *testing.T) {
	c := load(t, "qemu { cpus = 2 }\nqemu { cpus = 6 }\n")
	if c.QEMU.CPUs != 6 {
		t.Fatalf("cpus = %d", c.QEMU.CPUs)
	}
}

func TestMissingMainFile(t *testing.T) {
	_, err := Load(t.TempDir())
	if !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("want a not-exist error, got %v", err)
	}
}

func TestUnreadableLocalFileIsAnError(t *testing.T) {
	root := repo(t, "")
	if err := os.Mkdir(filepath.Join(root, LocalFile), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(root); err == nil || errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("a local settings path that cannot be read must fail, got %v", err)
	}
}

func TestSyntaxErrorsNameTheFile(t *testing.T) {
	if msg := loadErr(t, "qemu {\n"); !strings.HasPrefix(msg, "sextant.meri:1:") {
		t.Errorf("main file: %q", msg)
	}
	if msg := loadErr(t, "", "cpus = = 2\n"); !strings.HasPrefix(msg, "sextant.local.meri:1:") {
		t.Errorf("local file: %q", msg)
	}
}

func TestWrongTypesAreReported(t *testing.T) {
	msg := loadErr(t, "qemu {\n    memory = \"lots\"\n    cpus = many\n}\n")
	for _, want := range []string{"memory", "cpus"} {
		if !strings.Contains(msg, want) {
			t.Errorf("%q should mention %q", msg, want)
		}
	}
}

func TestRangeChecks(t *testing.T) {
	tests := []struct {
		doc, want string
	}{
		{"stats { min-ratio = -0.5 }", "stats.min-ratio must not be negative, got -0.5"},
		{"qemu { memory = 127MiB }", "qemu.memory must be at least 128MiB, got 133169152 bytes"},
		{"qemu { cpus = 0 }", "qemu.cpus must be between 1 and 64, got 0"},
		{"qemu { cpus = 65 }", "qemu.cpus must be between 1 and 64, got 65"},
		{"qemu { accel = vmware }", "qemu.accel must be one of [auto kvm hvf whpx tcg], got \"vmware\""},
		{"qemu { firmware = \"/a\" }", "set qemu.firmware and qemu.firmware-vars together, or neither"},
		{"qemu { firmware-vars = \"/b\" }", "set qemu.firmware and qemu.firmware-vars together, or neither"},
		{"qemu { timeout = 0s }", "qemu.timeout must be longer than zero"},
	}
	for _, tt := range tests {
		if msg := loadErr(t, tt.doc+"\n"); msg != tt.want {
			t.Errorf("%s:\ngot  %q\nwant %q", tt.doc, msg, tt.want)
		}
	}
}

func TestBoundariesAreAllowed(t *testing.T) {
	c := load(t, "stats { min-ratio = 0.0 }\nqemu {\n    memory = 128MiB\n    cpus = 1\n}\n")
	if c.MinRatio != 0 || c.QEMU.Memory != 128<<20 || c.QEMU.CPUs != 1 {
		t.Fatalf("got %+v", c)
	}
	c = load(t, "qemu {\n    cpus = 64\n    timeout = 1ms\n}\n")
	if c.QEMU.CPUs != 64 || c.QEMU.Timeout != time.Millisecond {
		t.Fatalf("got %+v", c)
	}
}

func TestEveryAcceleratorNameIsAccepted(t *testing.T) {
	for _, name := range []string{"auto", "kvm", "hvf", "whpx", "tcg"} {
		if c := load(t, "qemu { accel = "+name+" }\n"); c.QEMU.Accel != name {
			t.Errorf("accel %s read as %q", name, c.QEMU.Accel)
		}
	}
}

func TestAllProblemsAreReportedAtOnce(t *testing.T) {
	msg := loadErr(t, "qemu {\n    cpus = 0\n    accel = qemu\n    timeout = 0s\n}\n")
	if strings.Count(msg, "\n") != 2 {
		t.Fatalf("want three problems on three lines, got %q", msg)
	}
}

func TestTheRepositorysOwnSettingsLoad(t *testing.T) {
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	doc, err := read(filepath.Join(root, File))
	if err != nil {
		t.Fatalf("the committed sextant.meri does not parse: %v", err)
	}
	c, err := fromDocument(root, doc)
	if err != nil {
		t.Fatalf("the committed sextant.meri is not valid: %v", err)
	}
	if c.MinRatio != 1.5 {
		t.Fatalf("the project rule is 1.5, sextant.meri says %v", c.MinRatio)
	}
}
