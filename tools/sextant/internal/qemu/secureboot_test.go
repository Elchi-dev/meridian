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
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
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

// secureBoot boots a plain image whose loader is the given one on the
// Secure Boot firmware with the snakeoil key enrolled, until want appears.
func secureBoot(t *testing.T, r rig, loader []byte, want ...string) Result {
	t.Helper()
	files := bootFiles(t, false)
	files[image.LoaderPath] = loader
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
