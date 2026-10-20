package cli

import (
	"context"
	"crypto/rand"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
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

// built are the paths of what a build made: the loader, the kernel and
// the programs, these by their names on the image.
type built struct {
	loader, kernel string
	programs       map[string]string
}

// buildBoot builds the loader, the kernel and the programs. Cargo's
// progress goes to stderr, so stdout keeps only what the command reports.
func (s *session) buildBoot(ctx context.Context, v variant) (built, error) {
	loader, err := s.build(cargo.Loader, v).Run(ctx, s.env.Stderr, s.env.Stderr)
	if err != nil {
		return built{}, err
	}
	kernel, err := s.build(cargo.Kernel, v).Run(ctx, s.env.Stderr, s.env.Stderr)
	if err != nil {
		return built{}, err
	}
	b := built{loader: loader, kernel: kernel, programs: map[string]string{}}
	parts := cargo.Programs
	if v.testMode {
		parts = append(slices.Clone(parts), cargo.TestPrograms...)
	}
	for _, part := range parts {
		path, err := s.build(part, v).Run(ctx, s.env.Stderr, s.env.Stderr)
		if err != nil {
			return built{}, err
		}
		b.programs[part.File+image.ProgramExt] = path
	}
	return b, nil
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

// DevelopmentKeyFile is the name of this machine's development key in
// sextant's settings folder.
const DevelopmentKeyFile = "development-key.pem"

// signer is the key the loader is signed with: the one the settings name,
// or this machine's development key, made the first time it is needed.
func (s *session) signer() (authenticode.Signer, error) {
	path := s.settings.SignKey
	if path == "" {
		if s.env.ConfigDir == "" {
			return authenticode.Signer{}, errors.New("there is no settings folder to keep a development key in; set sign.key in sextant.local.meri")
		}
		path = filepath.Join(s.env.ConfigDir, "sextant", DevelopmentKeyFile)
		if _, err := os.Stat(path); errors.Is(err, fs.ErrNotExist) {
			return s.makeDevelopmentKey(path)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return authenticode.Signer{}, fmt.Errorf("the signing key: %w", err)
	}
	signer, err := authenticode.ParseSigner(data)
	if err != nil {
		return authenticode.Signer{}, fmt.Errorf("the signing key in %s: %w", path, err)
	}
	return signer, nil
}

// developmentKeyName names a machine's key after the machine, given what
// os.Hostname says.
func developmentKeyName(host string, err error) string {
	if err != nil || host == "" {
		host = "an unnamed machine"
	}
	return "Meridian development key, " + host
}

// makeDevelopmentKey makes a key for this machine and keeps it where only
// its owner can read it. It never replaces a key that is there.
func (s *session) makeDevelopmentKey(path string) (authenticode.Signer, error) {
	signer, err := authenticode.NewSigner(developmentKeyName(os.Hostname()), time.Now(), rand.Reader)
	if err != nil {
		return authenticode.Signer{}, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return authenticode.Signer{}, err
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return authenticode.Signer{}, fmt.Errorf("keep the development key: %w", err)
	}
	if _, err := f.Write(signer.PEM()); err != nil {
		f.Close()
		return authenticode.Signer{}, fmt.Errorf("keep the development key: %w", err)
	}
	if err := f.Close(); err != nil {
		return authenticode.Signer{}, err
	}
	fmt.Fprintf(s.env.Stderr, "sextant: made a development key for signing the loader: %s\n", path)
	return signer, nil
}

// bootFiles are the files of an image of this variant: the shim, the
// loader signed, and the certificate to enroll for it.
func (s *session) bootFiles(v variant, loader, kernel []byte, programs map[string][]byte) (map[string][]byte, error) {
	shim, err := image.ReadShim(s.root)
	if err != nil {
		return nil, err
	}
	licences, err := image.ReadFontLicences(s.root)
	if err != nil {
		return nil, err
	}
	signer, err := s.signer()
	if err != nil {
		return nil, err
	}
	signed, err := authenticode.Sign(loader, signer)
	if err != nil {
		return nil, fmt.Errorf("sign the loader: %w", err)
	}
	b := image.Boot{Loader: signed, Certificate: signer.Cert.Raw, Kernel: kernel, Shim: shim, FontLicences: licences, Programs: programs}
	if v.testMode {
		return image.TestFiles(b), nil
	}
	return image.BootFiles(b), nil
}

func (s *session) writeImage(ctx context.Context, v variant, b built, out string) error {
	loaderData, err := os.ReadFile(b.loader)
	if err != nil {
		return err
	}
	kernelData, err := os.ReadFile(b.kernel)
	if err != nil {
		return err
	}
	programs := map[string][]byte{}
	for name, path := range b.programs {
		if programs[name], err = os.ReadFile(path); err != nil {
			return err
		}
	}
	files, err := s.bootFiles(v, loaderData, kernelData, programs)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		return err
	}
	seed, when := s.identity(ctx)
	contents := image.Contents{Seed: seed, Time: when, Files: files}
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
	b, err := s.buildBoot(context.Background(), *v)
	if err != nil {
		return err
	}
	fmt.Fprintln(env.Stdout, b.loader)
	fmt.Fprintln(env.Stdout, b.kernel)
	for _, name := range slices.Sorted(maps.Keys(b.programs)) {
		fmt.Fprintln(env.Stdout, b.programs[name])
	}
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
	b, err := s.buildBoot(ctx, *v)
	if err != nil {
		return err
	}
	path := *out
	if path == "" {
		path = s.imagePath(*v)
	}
	if err := s.writeImage(ctx, *v, b, path); err != nil {
		return err
	}
	fmt.Fprintln(env.Stdout, path)
	return nil
}

// runSettings are what sextant run is asked for.
type runSettings struct {
	v            variant
	display, gdb bool
	// limit ends the machine after this long, and until once this text
	// has appeared on the serial port.
	limit time.Duration
	until string
}

func parseRun(args []string) (runSettings, error) {
	flags, v := variantFlags("run")
	headless := flags.Bool("headless", false, "no window; the serial port still goes to the terminal")
	debug := flags.Bool("debug", false, "wait for a debugger on localhost:1234 before starting")
	limit := flags.Duration("for", 24*time.Hour, "stop the machine after this long")
	until := flags.String("until", "", "stop the machine once this text appears on the serial port")
	if err := parseNoArgs(flags, args); err != nil {
		return runSettings{}, err
	}
	if *limit <= 0 {
		return runSettings{}, usageErrorf("--for takes a time to run, such as 30s, got %v", *limit)
	}
	return runSettings{v: *v, display: !*headless, gdb: *debug, limit: *limit, until: *until}, nil
}

func runRun(env Env, args []string) error {
	r, err := parseRun(args)
	if err != nil {
		return err
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	ctx := context.Background()
	b, err := s.buildBoot(ctx, r.v)
	if err != nil {
		return err
	}
	path := s.imagePath(r.v)
	if err := s.writeImage(ctx, r.v, b, path); err != nil {
		return err
	}
	m, err := s.machine(r.v, path)
	if err != nil {
		return err
	}
	m.Display = r.display
	m.GDB = r.gdb
	if r.gdb {
		fmt.Fprintln(env.Stderr, "Waiting for a debugger: gdb -ex 'target remote localhost:1234'")
	}
	opts := qemu.Options{Timeout: r.limit, Live: env.Stdout}
	if r.until != "" {
		// Nobody watches a run that waits for text, so a firmware that
		// stalls before printing anything is restarted, as for the tests.
		opts.Script = []qemu.Step{{Wait: r.until}}
		opts.StopAfterScript = true
		opts.FirmwareStall = s.settings.QEMU.Stall
		opts.Attempts = 3
	}
	res, err := qemu.Run(ctx, m, opts)
	return runOutcome(r, res, err)
}

// runOutcome is how a run ended for the person who started it: QEMU closed
// with 0, the time asked for over, or the text waited for seen.
func runOutcome(r runSettings, res qemu.Result, err error) error {
	switch {
	case err != nil:
		return err
	case r.until != "" && res.StepsDone == 0:
		return fmt.Errorf("%q did not appear on the serial port", r.until)
	case res.TimedOut || res.Stopped:
		return nil
	case res.ExitCode != 0:
		return fmt.Errorf("QEMU exited with %d", res.ExitCode)
	}
	return nil
}

// testScope says which tests sextant test runs.
type testScope struct {
	host, inQEMU bool
}

// parseTest says which tests sextant test runs: those on this machine,
// those in QEMU, or both when neither is asked for.
func parseTest(args []string) (testScope, error) {
	flags := flag.NewFlagSet("test", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	hostOnly := flags.Bool("host", false, "only the tests that run on this machine")
	qemuOnly := flags.Bool("qemu", false, "only the tests that boot an image in QEMU")
	if err := parseNoArgs(flags, args); err != nil {
		return testScope{}, err
	}
	both := !*hostOnly && !*qemuOnly
	return testScope{host: *hostOnly || both, inQEMU: *qemuOnly || both}, nil
}

func runTest(env Env, args []string) error {
	scope, err := parseTest(args)
	if err != nil {
		return err
	}
	s, err := openSession(env)
	if err != nil {
		return err
	}
	if scope.host && os.Getenv(insideChecks) != "" {
		return errors.New("sextant test is running inside its own host tests, which would start them again without end")
	}
	ctx := context.Background()
	if scope.host {
		if err := runSteps(ctx, s.hostSteps(), s.env.Stdout, s.env.Stderr); err != nil {
			return err
		}
	}
	if scope.inQEMU {
		return s.qemuTests(ctx)
	}
	return nil
}

// step is one command and the folder it runs in, with additions to the
// environment.
type step struct {
	dir  string
	name string
	args []string
	env  []string
}

// insideChecks is set for the commands sextant ci and the host tests run.
// Some of sextant's own tests run sextant ci or sextant test, so without
// it a mistake there would start the checks inside themselves without
// end, a thousand test processes deep.
const insideChecks = "SEXTANT_INSIDE_CHECKS"

// hostSteps are the tests that run on this machine: Cargo's for every
// crate, then Go's for sextant.
func (s *session) hostSteps() []step {
	inside := insideChecks + "=1"
	return []step{
		{s.root, "cargo", []string{"test", "--workspace", "--locked"}, []string{"CARGO_TARGET_DIR=" + s.settings.BuildDir, inside}},
		{filepath.Join(s.root, "tools", "sextant"), "go", []string{"test", "./..."}, []string{inside}},
	}
}

// runSteps runs the steps one after the other, each named first, and stops
// at the first that fails.
func runSteps(ctx context.Context, steps []step, stdout, stderr io.Writer) error {
	for _, st := range steps {
		fmt.Fprintf(stdout, "== %s %s\n", st.name, strings.Join(st.args, " "))
		cmd := exec.CommandContext(ctx, st.name, st.args...)
		cmd.Dir = st.dir
		cmd.Env = append(os.Environ(), st.env...)
		cmd.Stdout, cmd.Stderr = stdout, stderr
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("%s %s failed: %w", st.name, strings.Join(st.args, " "), err)
		}
	}
	return nil
}

// qemuTests boots the QEMU test image twice: on a machine as QEMU gives
// it, and on one without an x2APIC, XSAVE or a PS/2 controller, so the
// kernel's xAPIC and FXSAVE paths are checked as well wherever KVM would
// pass the newer ones through, and so is a keyboard that does not answer.
func (s *session) qemuTests(ctx context.Context) error {
	v := variant{testMode: true}
	b, err := s.buildBoot(ctx, v)
	if err != nil {
		return err
	}
	path := s.imagePath(v)
	if err := s.writeImage(ctx, v, b, path); err != nil {
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
	for _, pass := range testPasses(m, screens) {
		fmt.Fprintln(s.env.Stdout, pass.title)
		if err := s.testMachine(ctx, pass.machine, pass.screens); err != nil {
			return err
		}
	}
	return nil
}

// testPass is one boot of the test image: its heading, the machine it
// boots on, and where the screens it asks for go, if anywhere.
type testPass struct {
	title   string
	machine qemu.Machine
	screens string
}

// testPasses are the boots of the test image on m: first on a machine as
// QEMU gives it, keeping its screens in screens, then on one without an
// x2APIC, XSAVE or a PS/2 controller.
func testPasses(m qemu.Machine, screens string) []testPass {
	without := m
	without.WithoutX2APIC = true
	without.WithoutXSAVE = true
	without.WithoutPS2 = true
	return []testPass{
		{title: "== QEMU test image", machine: m, screens: screens},
		{title: "== QEMU test image without x2APIC, XSAVE or PS/2", machine: without},
	}
}

// testMachine boots the test image on m, prints each check, and keeps the
// screens it asks for in screens unless that is empty.
func (s *session) testMachine(ctx context.Context, m qemu.Machine, screens string) error {
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
