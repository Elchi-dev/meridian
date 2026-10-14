package doctor

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
)

// path makes a folder of empty programs with these names and puts it
// alone on the PATH, so the real lookup finds exactly them.
func path(t *testing.T, names ...string) {
	t.Helper()
	dir := t.TempDir()
	for _, n := range names {
		if runtime.GOOS == "windows" {
			n += ".exe"
		}
		if err := os.WriteFile(filepath.Join(dir, n), nil, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", dir)
}

func TestTheFirstManagerOnThePathIsTheOne(t *testing.T) {
	path(t, "brew", "dnf", "zypper")
	if got := Detect(exec.LookPath); got != Dnf {
		t.Errorf("got %q, want dnf before zypper and brew", got)
	}
	path(t, "winget")
	if got := Detect(exec.LookPath); got != Winget {
		t.Errorf("got %q", got)
	}
	path(t)
	if got := Detect(exec.LookPath); got != "" {
		t.Errorf("an empty PATH: %q", got)
	}
	if !slices.Equal(Managers, []Manager{Pacman, Apt, Dnf, Zypper, Brew, Winget}) {
		t.Error("the order the managers are looked for in")
	}
}

func TestEachManagerInstallsItsWay(t *testing.T) {
	pkgs := []string{"a", "b"}
	cases := map[Manager]string{
		Pacman: "sudo pacman -S --needed --noconfirm a b",
		Apt:    "sudo apt-get install -y a b",
		Dnf:    "sudo dnf install -y a b",
		Zypper: "sudo zypper install -y a b",
		Brew:   "brew install a b",
		Winget: "winget install --exact --id a|winget install --exact --id b",
	}
	for m, want := range cases {
		var lines []string
		for _, c := range InstallCommands(m, pkgs) {
			lines = append(lines, strings.Join(c, " "))
		}
		if got := strings.Join(lines, "|"); got != want {
			t.Errorf("%s: %q", m, got)
		}
	}
	if InstallCommands(Pacman, nil) != nil || InstallCommands("ports", pkgs) != nil {
		t.Error("nothing to install, or a manager sextant does not know")
	}
	if got := InstallCommands(Apt, pkgs); &got[0][4] == &pkgs[0] {
		t.Error("the command must not share the caller's slice")
	}
}

func TestEveryToolCanBeInstalledOnTheLinuxManagers(t *testing.T) {
	seen := map[string]bool{}
	for _, tool := range Tools {
		if seen[tool.Command] || tool.Name == "" || tool.Why == "" {
			t.Errorf("%q is listed twice or without a name or a reason", tool.Command)
		}
		seen[tool.Command] = true
		for _, m := range []Manager{Pacman, Apt, Dnf, Zypper} {
			if tool.Packages[m] == "" {
				t.Errorf("%s has no package for %s", tool.Name, m)
			}
		}
	}
	var required []string
	for _, tool := range Tools {
		if tool.Required {
			required = append(required, tool.Command)
		}
	}
	if !slices.Equal(required, []string{"go", "rustup", "qemu-system-x86_64"}) {
		t.Errorf("required: %v", required)
	}
	for _, m := range []Manager{Pacman, Apt, Dnf, Zypper, Brew} {
		if Firmware[m] == "" {
			t.Errorf("no OVMF package for %s", m)
		}
	}
}

func TestVersionsAreReadAndCompared(t *testing.T) {
	cases := map[string]Version{"1.27": {1, 27, 0}, "1.27.3": {1, 27, 3}, "8.2.2": {8, 2, 2}, "1.28rc1": {1, 28, 0}, "9.1.0-rc0": {9, 1, 0}}
	for s, want := range cases {
		if got, ok := ParseVersion(s); !ok || got != want {
			t.Errorf("%q: %v %v", s, got, ok)
		}
	}
	for _, bad := range []string{"", "1", "1.2.3.4", "a.b", "1..2", ".1"} {
		if _, ok := ParseVersion(bad); ok {
			t.Errorf("%q should not read", bad)
		}
	}
	v := Version{1, 27, 3}
	for want, ok := range map[Version]bool{{1, 27, 3}: true, {1, 27, 4}: false, {1, 26, 9}: true, {1, 28, 0}: false, {0, 99, 99}: true, {2, 0, 0}: false} {
		if v.AtLeast(want) != ok {
			t.Errorf("%v at least %v: want %v", v, want, ok)
		}
	}
	if v.String() != "1.27.3" {
		t.Error(v.String())
	}
}

func TestToolsTellTheirVersions(t *testing.T) {
	if v, ok := GoVersion("go version go1.27.3 linux/amd64\n"); !ok || v != (Version{1, 27, 3}) {
		t.Errorf("go: %v %v", v, ok)
	}
	if v, ok := GoVersion("go version go1.22 darwin/arm64"); !ok || v != (Version{1, 22, 0}) {
		t.Errorf("go without a patch: %v", v)
	}
	if _, ok := GoVersion("no go here"); ok {
		t.Error("nothing to read")
	}
	out := "QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1.4)\nCopyright (c) 2003-2023\n"
	if v, ok := QEMUVersion(out); !ok || v != (Version{8, 2, 2}) {
		t.Errorf("qemu: %v %v", v, ok)
	}
	if v, ok := QEMUVersion("QEMU emulator version 9.1.0\n"); !ok || v != (Version{9, 1, 0}) {
		t.Errorf("qemu, nothing after: %v", v)
	}
	if _, ok := QEMUVersion("Copyright\nQEMU emulator version 8.2.2"); ok {
		t.Error("only the first line counts")
	}
}

func TestSettingsComeFromTheToolchainFiles(t *testing.T) {
	toml := "[toolchain]\nchannel = \"1.99.0\"\nprofile = \"minimal\"\n"
	if got := Setting(toml, "channel"); got != "1.99.0" {
		t.Errorf("toml: %q", got)
	}
	mod := "module example.com/x\n\ngo 1.27.0\n\ntoolchain go1.27.3\n"
	if got := Setting(mod, "go"); got != "1.27.0" {
		t.Errorf("go.mod: %q", got)
	}
	if Setting(toml, "chan") != "" || Setting(toml, "target") != "" || Setting("channel", "channel") != "" {
		t.Error("a key must match whole and have a value")
	}
}

func TestTheToolchainListNamesTheChannel(t *testing.T) {
	list := "stable-x86_64-unknown-linux-gnu\n1.99.0-x86_64-unknown-linux-gnu (default)\n"
	if !HasToolchain(list, "1.99.0") || !HasToolchain(list, "stable") {
		t.Error("both are installed")
	}
	if HasToolchain(list, "1.99") || HasToolchain(list, "1.98.0") || HasToolchain("", "1.99.0") {
		t.Error("only the whole channel counts")
	}
}
