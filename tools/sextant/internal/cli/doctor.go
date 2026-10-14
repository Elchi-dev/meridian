package cli

import (
	"bufio"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/doctor"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/qemu"
)

// finding is what doctor found out about one thing the machine needs.
type finding struct {
	name, what string
	ok         bool
	required   bool
	// install names the package that brings it, fix says what to do when
	// no package does.
	install string
	fix     string
}

func runDoctor(env Env, args []string) error {
	flags := flag.NewFlagSet("doctor", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	yes := flags.Bool("yes", false, "install what is missing without asking")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	manager := doctor.Detect(exec.LookPath)
	findings := s.examine(manager)
	report(env.Stdout, findings)
	commands, advice := plan(manager, findings)
	if len(commands) == 0 && len(advice) == 0 {
		fmt.Fprintln(env.Stdout, "Everything Meridian needs is here.")
		return nil
	}
	for _, a := range advice {
		fmt.Fprintln(env.Stdout, a)
	}
	if len(commands) > 0 {
		fmt.Fprintln(env.Stdout, "To install what is missing:")
		for _, c := range commands {
			fmt.Fprintln(env.Stdout, "  "+strings.Join(c, " "))
		}
		if *yes || confirm(env.Stdin, env.Stdout) {
			if err := install(commands, s.root, env.Stdout, env.Stderr); err != nil {
				return err
			}
			findings = s.examine(manager)
			report(env.Stdout, findings)
		}
	}
	if missing := missingRequired(findings); missing > 0 {
		return fmt.Errorf("%d %s Meridian needs %s missing", missing, plural(missing, "thing", "things"), plural(missing, "is", "are"))
	}
	return nil
}

// examine checks everything the machine needs, with manager's packages
// for what is missing.
func (s *session) examine(manager doctor.Manager) []finding {
	var out []finding
	for _, tool := range doctor.Tools {
		f := finding{name: tool.Name, what: tool.Why, required: tool.Required, install: tool.Packages[manager]}
		path, err := exec.LookPath(tool.Command)
		f.ok = err == nil
		if f.ok {
			f.what = s.describe(tool.Command, path, f.what)
		}
		if f.ok && tool.Command == "go" {
			f = goFinding(f, path)
		}
		out = append(out, f)
	}
	out = append(out, s.toolchainFinding(), s.firmwareFinding(manager))
	if runtime.GOOS == "linux" {
		out = append(out, kvmFinding("/dev/kvm"))
	}
	return out
}

// describe adds what a tool says about its version to why it is needed.
func (s *session) describe(command, path, why string) string {
	if command != "qemu-system-x86_64" {
		return why
	}
	out, _ := exec.Command(path, "--version").Output()
	if v, ok := doctor.QEMUVersion(string(out)); ok {
		return why + ", version " + v.String()
	}
	return why
}

// goFinding adds Go's version. A Go too old to fetch the one go.mod asks
// for could not have built this sextant, so any version that reads is
// fine.
func goFinding(f finding, path string) finding {
	out, _ := exec.Command(path, "version").Output()
	if v, ok := doctor.GoVersion(string(out)); ok {
		f.what += ", version " + v.String()
	} else {
		f.ok, f.fix = false, "go version printed nothing sextant can read"
	}
	return f
}

// toolchainFinding checks rustup has the toolchain rust-toolchain.toml
// pins.
func (s *session) toolchainFinding() finding {
	f := finding{name: "Rust toolchain", what: "compiles the loader and the kernel", required: true}
	file, _ := os.ReadFile(filepath.Join(s.root, "rust-toolchain.toml"))
	channel := doctor.Setting(string(file), "channel")
	if channel == "" {
		f.fix = "rust-toolchain.toml is missing or names no channel"
		return f
	}
	f.what += ", " + channel
	// Without rustup the list is empty, and so has no toolchain.
	out, _ := exec.Command("rustup", "toolchain", "list").Output()
	if f.ok = doctor.HasToolchain(string(out), channel); !f.ok {
		f.fix = "run 'rustup toolchain install' at the root of the repository; it reads rust-toolchain.toml"
	}
	return f
}

// firmwareFinding checks QEMU will find OVMF.
func (s *session) firmwareFinding(manager doctor.Manager) finding {
	f := finding{name: "OVMF", what: "the UEFI firmware QEMU boots", required: true, install: doctor.Firmware[manager]}
	binary, _ := exec.LookPath("qemu-system-x86_64")
	configured := qemu.Firmware{Code: s.settings.QEMU.Code, Vars: s.settings.QEMU.Vars}
	fw, err := qemu.FindFirmware(runtime.GOOS, binary, configured, qemu.FileExists)
	f.ok = err == nil
	if f.ok {
		f.what += ", " + fw.Code
	}
	return f
}

// kvmFinding checks this user may run machines with KVM at device.
func kvmFinding(device string) finding {
	f := finding{name: "KVM", what: "makes QEMU fast", fix: "add yourself to the kvm group with 'sudo usermod -aG kvm $USER' and log in again"}
	file, err := os.OpenFile(device, os.O_RDWR, 0)
	if err == nil {
		file.Close()
		f.ok = true
	} else if errors.Is(err, os.ErrNotExist) {
		f.fix = "turn on virtualisation in the firmware's settings; QEMU still works without it, only slower"
	}
	return f
}

func report(w io.Writer, findings []finding) {
	for _, f := range findings {
		switch {
		case f.ok:
			fmt.Fprintf(w, "ok       %s: %s\n", f.name, f.what)
		case f.required:
			fmt.Fprintf(w, "missing  %s: %s\n", f.name, f.what)
		default:
			fmt.Fprintf(w, "optional %s: %s\n", f.name, f.what)
		}
	}
}

// plan gathers the packages to install with manager into its commands,
// and says what to do about the rest.
func plan(manager doctor.Manager, findings []finding) (commands [][]string, advice []string) {
	var packages []string
	for _, f := range findings {
		switch {
		case f.ok:
		case f.fix != "":
			advice = append(advice, f.name+": "+f.fix)
		case f.install != "" && !contains(packages, f.install):
			packages = append(packages, f.install)
		case f.install == "":
			advice = append(advice, f.name+": install it by hand; sextant knows no package for it here")
		}
	}
	return doctor.InstallCommands(manager, packages), advice
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// confirm asks whether to install, and takes only y or yes.
func confirm(in io.Reader, out io.Writer) bool {
	fmt.Fprint(out, "Install it now? [y/N] ")
	line, _ := bufio.NewReader(in).ReadString('\n')
	answer := strings.ToLower(strings.TrimSpace(line))
	fmt.Fprintln(out)
	return answer == "y" || answer == "yes"
}

// install runs each command in dir, stopping at the first that fails.
func install(commands [][]string, dir string, stdout, stderr io.Writer) error {
	for _, c := range commands {
		fmt.Fprintln(stdout, "== "+strings.Join(c, " "))
		cmd := exec.Command(c[0], c[1:]...)
		cmd.Dir = dir
		cmd.Stdin = os.Stdin
		cmd.Stdout, cmd.Stderr = stdout, stderr
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("%s failed: %w", strings.Join(c, " "), err)
		}
	}
	return nil
}

func missingRequired(findings []finding) int {
	n := 0
	for _, f := range findings {
		if f.required && !f.ok {
			n++
		}
	}
	return n
}
