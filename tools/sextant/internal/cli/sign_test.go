package cli

import (
	"bytes"
	"crypto/rand"
	"debug/pe"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
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
	loader, kernel, err := s.buildBoot(t.Context(), variant{})
	if err != nil {
		t.Fatal(err)
	}
	loaderData, _ := os.ReadFile(loader)
	kernelData, _ := os.ReadFile(kernel)
	if certificateTableSize(t, loaderData) != 0 {
		t.Fatal("Cargo's loader should not be signed yet")
	}
	for _, v := range []variant{{}, {testMode: true}} {
		files, err := s.bootFiles(v, loaderData, kernelData)
		if err != nil {
			t.Fatal(err)
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
	}
}
