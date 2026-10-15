package qemu

import (
	"context"
	"crypto/rand"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/gpt"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/secureboot"
)

// Ubuntu's and Debian's OVMF ship a Secure Boot build with a template
// variable store that holds the "snakeoil" test key as PK, KEK and db, and
// that key itself, so whatever it signs passes the firmware's checks.
var snakeoil = struct{ code, vars, key, cert string }{
	code: "/usr/share/OVMF/OVMF_CODE_4M.secboot.fd",
	vars: "/usr/share/OVMF/OVMF_VARS_4M.snakeoil.fd",
	key:  "/usr/share/ovmf/PkKek-1-snakeoil.key",
	cert: "/usr/share/ovmf/PkKek-1-snakeoil.pem",
}

// snakeoilSigner reads the test key. It is stored encrypted with the
// password "snakeoil", as the package documents, and OpenSSL decrypts it,
// since Go's library reads no encrypted PKCS #8.
func snakeoilSigner(t *testing.T) authenticode.Signer {
	t.Helper()
	for _, f := range []string{snakeoil.code, snakeoil.vars, snakeoil.key, snakeoil.cert} {
		_, err := os.Stat(f)
		need(t, err == nil, f+" from Ubuntu's or Debian's ovmf package")
	}
	openssl, err := exec.LookPath("openssl")
	need(t, err == nil, "openssl")
	key, err := exec.Command(openssl, "pkey", "-in", snakeoil.key, "-passin", "pass:snakeoil").Output()
	if err != nil {
		t.Fatalf("decrypt the snakeoil key: %v", err)
	}
	cert, err := os.ReadFile(snakeoil.cert)
	if err != nil {
		t.Fatal(err)
	}
	s, err := authenticode.ParseSigner(append(key, cert...))
	if err != nil {
		t.Fatal(err)
	}
	return s
}

// secureBoot boots a plain image on the Secure Boot firmware with the
// snakeoil key enrolled, until want appears. The firmware starts the given
// loader itself, where the shim would be, since that key signed no shim.
func secureBoot(t *testing.T, r rig, loader []byte, want ...string) Result {
	t.Helper()
	files := bootFiles(t, false)
	files[image.FirmwarePath] = loader
	m := r.machine(diskWithFiles(t, files))
	m.DebugExit = false
	m.SecureBoot = true
	m.Firmware = Firmware{Code: snakeoil.code, Vars: snakeoil.vars}
	var script []Step
	for _, w := range want {
		script = append(script, Step{Wait: w})
	}
	res, err := Run(context.Background(), m, reliable(Options{
		Timeout:         2 * time.Minute,
		Script:          script,
		StopAfterScript: true,
	}))
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	return res
}

func TestSecureBootStartsALoaderSignedWithAnEnrolledKey(t *testing.T) {
	r := newRig(t)
	signer := snakeoilSigner(t)
	signed, err := authenticode.Sign(bootFiles(t, false)[image.LoaderPath], signer)
	if err != nil {
		t.Fatal(err)
	}
	res := secureBoot(t, r, signed, "Secure Boot: on", "Meridian kernel ")
	if res.StepsDone != 2 || !res.Stopped || res.TimedOut {
		t.Fatalf("steps %d, stopped %v, timed out %v\n%s", res.StepsDone, res.Stopped, res.TimedOut, res.Serial)
	}
}

func TestSecureBootRefusesOtherLoaders(t *testing.T) {
	r := newRig(t)
	snakeoilSigner(t)
	unsigned := bootFiles(t, false)[image.LoaderPath]
	stranger, err := authenticode.NewSigner("Meridian key nobody enrolled", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	foreign, err := authenticode.Sign(unsigned, stranger)
	if err != nil {
		t.Fatal(err)
	}
	for name, loader := range map[string][]byte{"unsigned": unsigned, "signed with another key": foreign} {
		res := secureBoot(t, r, loader, "Access Denied")
		if res.StepsDone != 1 || strings.Contains(res.Serial, "Meridian boot") {
			t.Errorf("%s: steps %d\n%s", name, res.StepsDone, res.Serial)
		}
	}
}

// OVMF built with Secure Boot and a variable store without keys, where
// the distributions put them: the firmware starts in Setup Mode.
var blankSecureBoot = []Firmware{
	{Code: "/usr/share/OVMF/OVMF_CODE_4M.secboot.fd", Vars: "/usr/share/OVMF/OVMF_VARS_4M.fd"},
	{Code: "/usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd", Vars: "/usr/share/edk2/x64/OVMF_VARS.4m.fd"},
	{Code: "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd", Vars: "/usr/share/edk2/ovmf/OVMF_VARS.fd"},
}

// The test image enrolls a key of Meridian's own from Setup Mode, with
// the writes sextant makes, and restarts. With Secure Boot then on, the
// firmware starts the loader signed with that key, and the loader sees
// the firmware refuse a copy of itself unsigned or signed with another
// key, and start one signed with the enrolled key.
// No distribution's key or template is involved.
func TestMeridiansOwnKeyIsEnrolledAndEnforced(t *testing.T) {
	r := newRig(t)
	var fw Firmware
	for _, f := range blankSecureBoot {
		if FileExists(f.Code) && FileExists(f.Vars) {
			fw = f
			break
		}
	}
	need(t, fw.Code != "", "OVMF built with Secure Boot, with a blank variable store")
	signer, err := authenticode.NewSigner("Meridian test key", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	files := bootFiles(t, true)
	unsigned := files[image.LoaderPath]
	signed, err := authenticode.Sign(unsigned, signer)
	if err != nil {
		t.Fatal(err)
	}
	stranger, err := authenticode.NewSigner("Meridian key nobody enrolled", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	foreign, err := authenticode.Sign(unsigned, stranger)
	if err != nil {
		t.Fatal(err)
	}
	// Only Meridian's key is enrolled, which signed no shim, so the firmware
	// starts the loader itself; the test image starts it again by its own
	// name.
	files[image.FirmwarePath] = signed
	files[image.LoaderPath] = signed
	files["EFI/unsigned/tool.efi"] = unsigned
	files["EFI/foreign/tool.efi"] = foreign
	owner, err := gpt.ParseGUID("0E6F5E4C-2A33-4B8C-9C0D-5D2E8E1A7F31")
	if err != nil {
		t.Fatal(err)
	}
	writes, err := secureboot.Enroll(signer, owner, time.Date(2026, 10, 14, 0, 0, 0, 0, time.UTC))
	if err != nil {
		t.Fatal(err)
	}
	for i, v := range secureboot.Databases {
		files["meridian/keys/"+v.Name+".auth"] = writes[i]
	}
	m := r.machine(diskWithFiles(t, files))
	m.SecureBoot = true
	m.Firmware = fw
	m.Reboot = true
	res, err := Run(context.Background(), m, reliable(Options{Timeout: 2 * time.Minute, AnswerPress: true}))
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if err := ParseReport(res.Serial).Verdict(res.ExitCode, res.TimedOut, LastLine(res.Serial)); err != nil {
		t.Fatalf("%v\n%s", err, res.Serial)
	}
	for _, name := range []string{"db", "kek", "pk", "user_mode", "secure_boot", "unsigned_refused", "foreign_refused", "signed_started"} {
		if !strings.Contains(res.Serial, "TEST boot.enroll."+name+" ok") {
			t.Errorf("no boot.enroll.%s in:\n%s", name, res.Serial)
		}
	}
}

// OVMF built with Secure Boot and Microsoft's keys enrolled, as a PC from
// a shop has them, where the distributions put it.
var microsoftSecureBoot = []Firmware{
	{Code: "/usr/share/OVMF/OVMF_CODE_4M.secboot.fd", Vars: "/usr/share/OVMF/OVMF_VARS_4M.ms.fd"},
	{Code: "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd", Vars: "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd"},
}

// On firmware with Microsoft's keys the shim starts, refuses the loader,
// whose key nobody enrolled, and starts MokManager. sextant presses
// through enrolling the certificate on the image, as a person would, and
// after the restart the shim starts the loader, and the loader the kernel,
// with Secure Boot on.
func TestTheShimStartsTheLoaderOnceItsKeyIsEnrolled(t *testing.T) {
	r := newRig(t)
	var fw Firmware
	for _, f := range microsoftSecureBoot {
		if FileExists(f.Code) && FileExists(f.Vars) {
			fw = f
			break
		}
	}
	need(t, fw.Code != "", "OVMF built with Secure Boot, with Microsoft's keys enrolled")
	signer, err := authenticode.NewSigner("Meridian test key", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	files := bootFiles(t, false)
	signed, err := authenticode.Sign(files[image.LoaderPath], signer)
	if err != nil {
		t.Fatal(err)
	}
	files[image.LoaderPath] = signed
	files[image.CertificatePath] = signer.Cert.Raw
	m := r.machine(diskWithFiles(t, files))
	m.DebugExit = false
	m.SecureBoot = true
	m.Firmware = fw
	m.Reboot = true
	// MokManager drops keys that come while it draws, so they come at the
	// pace of a person: the first once the text is there, then one after
	// another with a pause.
	var script []Step
	press := func(wait string, keys ...string) {
		script = append(script, Step{Wait: wait, Pause: 500 * time.Millisecond, Keys: keys[:1]})
		for _, k := range keys[1:] {
			script = append(script, Step{Pause: 400 * time.Millisecond, Keys: []string{k}})
		}
	}
	press("Verification failed", "ret")
	press("to perform MOK management", "spc")
	press("Enroll key from disk", "down", "ret")
	press("MERIDIAN", "ret")
	// The partition's top holds EFI/, then the certificate.
	press("meridian.cer", "down", "ret")
	press("View key 0", "down", "ret")
	press("Enroll the key(s)?", "down", "ret")
	press("Reboot", "ret")
	script = append(script, Step{Wait: "Meridian boot"}, Step{Wait: "Secure Boot: on"}, Step{Wait: "Meridian kernel "})
	res, err := Run(context.Background(), m, reliable(Options{
		Timeout:         3 * time.Minute,
		Script:          script,
		StopAfterScript: true,
	}))
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if res.StepsDone != len(script) || !res.Stopped || res.TimedOut {
		t.Fatalf("steps %d of %d, stopped %v, timed out %v\n%s", res.StepsDone, len(script), res.Stopped, res.TimedOut, Clean(res.Serial))
	}
	if n := strings.Count(Clean(res.Serial), "Meridian boot"); n != 1 {
		t.Errorf("the loader should start once, after its key was enrolled, not %d times", n)
	}
}
