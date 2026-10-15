package cli

import (
	"bytes"
	"crypto/rand"
	"debug/pe"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/cargo"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
)

// testConfigDir stands in for the user's settings folder in every test,
// so no test makes a key in the real one.
var testConfigDir string

func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "sextant-config-")
	if err != nil {
		panic(err)
	}
	testConfigDir = dir
	code := m.Run()
	os.RemoveAll(dir)
	os.Exit(code)
}

func signingSession(t *testing.T) (*session, *strings.Builder) {
	t.Helper()
	var out strings.Builder
	s := testSession(t.TempDir())
	s.env = Env{Stdout: &out, Stderr: &out, ConfigDir: t.TempDir()}
	return s, &out
}

func TestTheDevelopmentKeyIsMadeOnceAndKept(t *testing.T) {
	s, out := signingSession(t)
	first, err := s.signer()
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(s.env.ConfigDir, "sextant", DevelopmentKeyFile)
	if !strings.Contains(out.String(), "made a development key") || !strings.Contains(out.String(), path) {
		t.Errorf("making the key should be reported: %q", out.String())
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" && info.Mode().Perm() != 0o600 {
		t.Errorf("the key file is %v; only its owner should read it", info.Mode().Perm())
	}
	if !strings.HasPrefix(first.Cert.Subject.CommonName, "Meridian development key, ") {
		t.Errorf("the certificate is named %q", first.Cert.Subject.CommonName)
	}
	out.Reset()
	second, err := s.signer()
	if err != nil {
		t.Fatal(err)
	}
	if !second.Cert.Equal(first.Cert) || out.Len() != 0 {
		t.Fatalf("the second time the same key should be read back quietly: %q", out.String())
	}
}

func TestTheKeyInTheSettingsWins(t *testing.T) {
	s, out := signingSession(t)
	release, err := authenticode.NewSigner("Meridian release key", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	s.settings.SignKey = filepath.Join(t.TempDir(), "release.pem")
	if err := os.WriteFile(s.settings.SignKey, release.PEM(), 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := s.signer()
	if err != nil {
		t.Fatal(err)
	}
	if !got.Cert.Equal(release.Cert) {
		t.Fatalf("signed with %q", got.Cert.Subject.CommonName)
	}
	if out.Len() != 0 {
		t.Fatalf("no development key should be made: %q", out.String())
	}
	if entries, _ := os.ReadDir(s.env.ConfigDir); len(entries) != 0 {
		t.Fatalf("the settings folder should stay empty: %v", entries)
	}
}

func TestSigningWithoutAKeyIsExplained(t *testing.T) {
	s, _ := signingSession(t)
	s.env.ConfigDir = ""
	if _, err := s.signer(); err == nil || !strings.Contains(err.Error(), "set sign.key in sextant.local.meri") {
		t.Errorf("no settings folder: %v", err)
	}
	s.settings.SignKey = filepath.Join(t.TempDir(), "missing.pem")
	if _, err := s.signer(); err == nil || !strings.Contains(err.Error(), "the signing key") {
		t.Errorf("a missing key file: %v", err)
	}
	if err := os.WriteFile(s.settings.SignKey, []byte("no key here"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := s.signer(); err == nil || !strings.Contains(err.Error(), "missing.pem") {
		t.Errorf("a file without a key: %v", err)
	}
}

func TestADevelopmentKeyIsNeverReplaced(t *testing.T) {
	s, _ := signingSession(t)
	path := filepath.Join(s.env.ConfigDir, "sextant", DevelopmentKeyFile)
	if _, err := s.makeDevelopmentKey(path); err != nil {
		t.Fatal(err)
	}
	before, _ := os.ReadFile(path)
	if _, err := s.makeDevelopmentKey(path); err == nil {
		t.Fatal("making a key where one is should fail")
	}
	after, _ := os.ReadFile(path)
	if !bytes.Equal(before, after) {
		t.Fatal("the key on disk changed")
	}
}

// certificateTableSize is the size of the certificate table in a PE image,
// read by Go's own PE reader.
func certificateTableSize(t *testing.T, b []byte) uint32 {
	t.Helper()
	f, err := pe.NewFile(bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	return f.OptionalHeader.(*pe.OptionalHeader64).DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_SECURITY].Size
}

func TestImagesCarryASignedLoader(t *testing.T) {
	needTool(t, "cargo")
	s, _ := signingSession(t)
	s.root = realRoot(t)
	s.settings.BuildDir = filepath.Join(os.TempDir(), "sextant-test-build")
	b, err := s.buildBoot(t.Context(), variant{})
	if err != nil {
		t.Fatal(err)
	}
	loaderData, _ := os.ReadFile(b.loader)
	kernelData, _ := os.ReadFile(b.kernel)
	hello, _ := os.ReadFile(b.programs["hello"])
	if !bytes.HasPrefix(hello, []byte("\x7fELF")) {
		t.Fatalf("hello was not built: %d bytes from %q", len(hello), b.programs["hello"])
	}
	if certificateTableSize(t, loaderData) != 0 {
		t.Fatal("Cargo's loader should not be signed yet")
	}
	signer, err := s.signer()
	if err != nil {
		t.Fatal(err)
	}
	shim, err := image.ReadShim(s.root)
	if err != nil {
		t.Fatal(err)
	}
	for _, v := range []variant{{}, {testMode: true}} {
		files, err := s.bootFiles(v, loaderData, kernelData, map[string][]byte{"hello": hello})
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(files[image.FirmwarePath], shim.Shim) || !bytes.Equal(files[image.ManagerPath], shim.Manager) {
			t.Errorf("test mode %v: the shim and MokManager should be on the image as the repository has them", v.testMode)
		}
		if !bytes.Equal(files[image.CertificatePath], signer.Cert.Raw) {
			t.Errorf("test mode %v: the certificate to enroll should be the one of the key that signed the loader", v.testMode)
		}
		signed := files[image.LoaderPath]
		if certificateTableSize(t, signed) == 0 {
			t.Errorf("test mode %v: the loader on the image is not signed", v.testMode)
		}
		// Signing pads the loader to 8 bytes; the digest covers the padding.
		padded := append(bytes.Clone(loaderData), make([]byte, (8-len(loaderData)%8)%8)...)
		want, _ := authenticode.Digest(padded)
		if got, err := authenticode.Digest(signed); err != nil || got != want {
			t.Errorf("test mode %v: the signed loader's digest %x, %v", v.testMode, got, err)
		}
		if !bytes.Equal(files[image.KernelPath], kernelData) {
			t.Errorf("test mode %v: the kernel should be on the image as built", v.testMode)
		}
		if !bytes.Equal(files[image.ProgramsPath+"/hello"], hello) {
			t.Errorf("test mode %v: hello should be on the image as built", v.testMode)
		}
	}
}

// The shim reads the loader's SBAT data from its .sbat section before it
// starts it, so the signed loader on the image has to carry it, in release
// builds too, where link-time optimisation drops what nothing uses.
func TestTheSignedLoaderCarriesItsSBAT(t *testing.T) {
	needTool(t, "cargo")
	s, _ := signingSession(t)
	s.root = realRoot(t)
	s.settings.BuildDir = filepath.Join(os.TempDir(), "sextant-test-build")
	for _, v := range []variant{{}, {release: true}} {
		loader, err := s.build(cargo.Loader, v).Run(t.Context(), io.Discard, io.Discard)
		if err != nil {
			t.Fatal(err)
		}
		loaderData, _ := os.ReadFile(loader)
		files, err := s.bootFiles(v, loaderData, nil, nil)
		if err != nil {
			t.Fatal(err)
		}
		checkSBAT(t, files[image.LoaderPath], v.release)
	}
}

func checkSBAT(t *testing.T, loader []byte, release bool) {
	t.Helper()
	f, err := pe.NewFile(bytes.NewReader(loader))
	if err != nil {
		t.Fatal(err)
	}
	section := f.Section(".sbat")
	if section == nil {
		t.Fatalf("release %v: the loader has no .sbat section", release)
	}
	if section.Characteristics&pe.IMAGE_SCN_MEM_WRITE != 0 || section.Characteristics&pe.IMAGE_SCN_MEM_EXECUTE != 0 {
		t.Errorf("the .sbat section should be read-only data, has %#x", section.Characteristics)
	}
	data, err := section.Data()
	if err != nil {
		t.Fatal(err)
	}
	text := string(data[:section.VirtualSize])
	lines := strings.Split(strings.TrimSuffix(text, "\n"), "\n")
	if len(lines) != 2 ||
		lines[0] != "sbat,1,SBAT Version,sbat,1,https://github.com/rhboot/shim/blob/main/SBAT.md" ||
		!strings.HasPrefix(lines[1], "meridian,1,Meridian,meridian,") ||
		!strings.HasSuffix(lines[1], ",https://github.com/Elchi-dev/meridian") {
		t.Fatalf("release %v: the .sbat section says %q", release, text)
	}
}

func TestImagesNeedTheShim(t *testing.T) {
	s, out := signingSession(t)
	_, err := s.bootFiles(variant{}, []byte("MZ"), nil, nil)
	if err == nil || !strings.Contains(err.Error(), "decision record 0013") {
		t.Fatalf("an image without the shim: %v", err)
	}
	if out.Len() != 0 {
		t.Errorf("no key should be made for an image that cannot be written: %q", out.String())
	}
}
