package cli

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/cargo"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/project"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/qemu"
)

// session is what every command that builds or boots Meridian starts from.
type session struct {
	env      Env
	root     string
	settings config.Config
}

func openSession(env Env) (*session, error) {
	root, err := project.FindRoot(env.Dir)
	if err != nil {
		return nil, err
	}
	settings, err := config.Load(root)
	if err != nil {
		return nil, err
	}
	return &session{env: env, root: root, settings: settings}, nil
}

// variant is the set of flags build, image, run and test share.
type variant struct {
	release  bool
	testMode bool
	iso      bool
}

func (s *session) build(part cargo.Part, v variant) cargo.Build {
	return cargo.Build{Part: part, Root: s.root, BuildDir: s.settings.BuildDir, Release: v.release, QEMUTest: v.testMode}
}

// buildBoot builds the loader and the kernel and returns their paths.
// Cargo's progress goes to stderr, so stdout keeps only what the command
// reports.
func (s *session) buildBoot(ctx context.Context, v variant) (loader, kernel string, err error) {
	loader, err = s.build(cargo.Loader, v).Run(ctx, s.env.Stderr, s.env.Stderr)
	if err != nil {
		return "", "", err
	}
	kernel, err = s.build(cargo.Kernel, v).Run(ctx, s.env.Stderr, s.env.Stderr)
	return loader, kernel, err
}

// imagePath is where an image of this variant is written by default.
func (s *session) imagePath(v variant) string {
	name := "meridian"
	if v.testMode {
		name += "-test"
	}
	if v.iso {
		return filepath.Join(s.settings.BuildDir, name+".iso")
	}
	return filepath.Join(s.settings.BuildDir, name+".img")
}

// identity is the seed and time stamped into images: the commit the
// checkout is at, so the same commit always gives the same image. Outside a
// git checkout, such as an unpacked release, both fall back to fixed values.
func (s *session) identity(ctx context.Context) (string, time.Time) {
	seed := "meridian"
	cmd := exec.CommandContext(ctx, "git", "rev-parse", "HEAD")
	cmd.Dir = s.root
	if out, err := cmd.Output(); err == nil {
		seed += " " + strings.TrimSpace(string(out))
	}
	when, err := image.BuildTime(ctx, s.root, os.Getenv)
	if err != nil {
		when = time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	}
	return seed, when
}

func (s *session) writeImage(ctx context.Context, v variant, loader, kernel, out string) error {
	loaderData, err := os.ReadFile(loader)
	if err != nil {
		return err
	}
	kernelData, err := os.ReadFile(kernel)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		return err
	}
	seed, when := s.identity(ctx)
	contents := image.Contents{Seed: seed, Time: when, Files: image.BootFiles(loaderData, kernelData)}
	if v.iso {
		return image.ISO(out, contents)
	}
	return image.Disk(out, contents)
}

func (s *session) machine(v variant, path string) (qemu.Machine, error) {
	binary, err := exec.LookPath("qemu-system-x86_64")
	if err != nil {
		return qemu.Machine{}, errors.New("qemu-system-x86_64 is not installed or not on the PATH; run 'sextant doctor' to install it")
	}
	q := s.settings.QEMU
	fw, err := qemu.FindFirmware(runtime.GOOS, binary, qemu.Firmware{Code: q.Code, Vars: q.Vars}, qemu.FileExists)
	if err != nil {
		return qemu.Machine{}, err
	}
	m := qemu.Machine{
		Binary:    binary,
		Firmware:  fw,
		Memory:    q.Memory,
		CPUs:      q.CPUs,
		Accels:    qemu.Accelerators(q.Accel, runtime.GOOS, runtime.GOARCH),
		DebugExit: v.testMode,
	}
	if v.iso {
		m.CDROM = path
	} else {
		m.Disk = path
	}
	return m, nil
}

func variantFlags(name string) (*flag.FlagSet, *variant) {
	flags := flag.NewFlagSet(name, flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	v := &variant{}
	flags.BoolVar(&v.release, "release", false, "build with optimisations")
	flags.BoolVar(&v.iso, "iso", false, "an ISO image instead of a disk image")
	return flags, v
}

func parseNoArgs(flags *flag.FlagSet, args []string) error {
	if err := flags.Parse(args); err != nil {
		return usageErrorf("%v", err)
	}
	if flags.NArg() > 0 {
		return usageErrorf("%s takes no arguments, got %q", flags.Name(), flags.Arg(0))
	}
	return nil
}

func runBuild(env Env, args []string) error {
	flags, v := variantFlags("build")
	flags.BoolVar(&v.testMode, "test-image", false, "build the QEMU test image instead")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	if v.iso {
		return usageErrorf("build makes the loader and the kernel; use 'sextant image --iso' for an ISO")
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	loader, kernel, err := s.buildBoot(context.Background(), *v)
	if err != nil {
		return err
	}
	fmt.Fprintln(env.Stdout, loader)
	fmt.Fprintln(env.Stdout, kernel)
	return nil
}

func runImage(env Env, args []string) error {
	flags, v := variantFlags("image")
	flags.BoolVar(&v.testMode, "test-image", false, "write the QEMU test image instead")
	out := flags.String("out", "", "where to write the image")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	ctx := context.Background()
	loader, kernel, err := s.buildBoot(ctx, *v)
	if err != nil {
		return err
	}
	path := *out
	if path == "" {
		path = s.imagePath(*v)
	}
	if err := s.writeImage(ctx, *v, loader, kernel, path); err != nil {
		return err
	}
	fmt.Fprintln(env.Stdout, path)
	return nil
}

func runRun(env Env, args []string) error {
	flags, v := variantFlags("run")
	headless := flags.Bool("headless", false, "no window; the serial port still goes to the terminal")
	debug := flags.Bool("debug", false, "wait for a debugger on localhost:1234 before starting")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	ctx := context.Background()
	loader, kernel, err := s.buildBoot(ctx, *v)
	if err != nil {
		return err
	}
	path := s.imagePath(*v)
	if err := s.writeImage(ctx, *v, loader, kernel, path); err != nil {
		return err
	}
	m, err := s.machine(*v, path)
	if err != nil {
		return err
	}
	m.Display = !*headless
	m.GDB = *debug
	if *debug {
		fmt.Fprintln(env.Stderr, "Waiting for a debugger: gdb -ex 'target remote localhost:1234'")
	}
	res, err := qemu.Run(ctx, m, qemu.Options{Timeout: 24 * time.Hour, Live: env.Stdout})
	if err != nil {
		return err
	}
	if res.ExitCode != 0 {
		return fmt.Errorf("QEMU exited with %d", res.ExitCode)
	}
	return nil
}

func runTest(env Env, args []string) error {
	flags := flag.NewFlagSet("test", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	host := flags.Bool("host", false, "only the tests that run on this machine")
	inQEMU := flags.Bool("qemu", false, "only the tests that boot an image in QEMU")
	if err := parseNoArgs(flags, args); err != nil {
		return err
	}
	all := !*host && !*inQEMU
	s, err := openSession(env)
	if err != nil {
		return err
	}
	ctx := context.Background()
	if *host || all {
		if err := s.hostTests(ctx); err != nil {
			return err
		}
	}
	if *inQEMU || all {
		return s.qemuTests(ctx)
	}
	return nil
}

func (s *session) hostTests(ctx context.Context) error {
	steps := []struct {
		dir  string
		name string
		args []string
		env  []string
	}{
		{s.root, "cargo", []string{"test", "--workspace", "--locked"}, []string{"CARGO_TARGET_DIR=" + s.settings.BuildDir}},
		{filepath.Join(s.root, "tools", "sextant"), "go", []string{"test", "./..."}, nil},
	}
	for _, step := range steps {
		fmt.Fprintf(s.env.Stdout, "== %s %s\n", step.name, strings.Join(step.args, " "))
		cmd := exec.CommandContext(ctx, step.name, step.args...)
		cmd.Dir = step.dir
		cmd.Env = append(os.Environ(), step.env...)
		cmd.Stdout, cmd.Stderr = s.env.Stdout, s.env.Stderr
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("%s %s failed: %w", step.name, strings.Join(step.args, " "), err)
		}
	}
	return nil
}

func (s *session) qemuTests(ctx context.Context) error {
	v := variant{testMode: true}
	fmt.Fprintln(s.env.Stdout, "== QEMU test image")
	loader, kernel, err := s.buildBoot(ctx, v)
	if err != nil {
		return err
	}
	path := s.imagePath(v)
	if err := s.writeImage(ctx, v, loader, kernel, path); err != nil {
		return err
	}
	m, err := s.machine(v, path)
	if err != nil {
		return err
	}
	screens := filepath.Join(s.settings.BuildDir, "screens")
	if err := os.MkdirAll(screens, 0o755); err != nil {
		return err
	}
	res, err := qemu.Run(ctx, m, qemu.Options{
		Timeout:       s.settings.QEMU.Timeout,
		AnswerPress:   true,
		Screens:       screens,
		Live:          s.env.Stderr,
		FirmwareStall: s.settings.QEMU.Stall,
		Attempts:      3,
	})
	if err != nil {
		return err
	}
	if res.FirmwareStalled {
		return fmt.Errorf("the firmware printed nothing in %d attempts; check that %s boots by hand", res.Attempts, m.Firmware.Code)
	}
	report := qemu.ParseReport(res.Serial)
	for _, name := range report.Passed {
		fmt.Fprintf(s.env.Stdout, "ok      %s\n", name)
	}
	for _, f := range report.Failed {
		fmt.Fprintf(s.env.Stdout, "FAILED  %s: %s\n", f.Name, f.Why)
	}
	for _, shot := range res.Screens {
		fmt.Fprintf(s.env.Stdout, "screen  %s\n", shot)
	}
	if err := report.Verdict(res.ExitCode, res.TimedOut, qemu.LastLine(res.Serial)); err != nil {
		return err
	}
	fmt.Fprintf(s.env.Stdout, "%d checks passed in QEMU\n", len(report.Passed))
	return nil
}
