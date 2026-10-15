package cli

import (
	"bytes"
	"crypto/sha256"
	"crypto/x509"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
)

func TestKeyNewMakesTheReleaseKeyOnce(t *testing.T) {
	dir := t.TempDir()
	r := runIn(dir, "key", "new", "release.pem")
	if r.code != ExitOK {
		t.Fatalf("exit %d: %s", r.code, r.stderr)
	}
	keyPath, certPath := filepath.Join(dir, "release.pem"), filepath.Join(dir, "release.cer")
	data, err := os.ReadFile(keyPath)
	if err != nil {
		t.Fatal(err)
	}
	signer, err := authenticode.ParseSigner(data)
	if err != nil {
		t.Fatal(err)
	}
	if signer.Cert.Subject.CommonName != "Meridian release key" {
		t.Errorf("the certificate is for %q", signer.Cert.Subject.CommonName)
	}
	cert, err := os.ReadFile(certPath)
	if err != nil || !bytes.Equal(cert, signer.Cert.Raw) {
		t.Errorf("the certificate alone should be the key's certificate: %v", err)
	}
	if _, err := signer.SignDetached([]byte("a release")); err != nil {
		t.Errorf("the key should sign: %v", err)
	}
	if runtime.GOOS != "windows" {
		if info, _ := os.Stat(keyPath); info.Mode().Perm() != 0o600 {
			t.Errorf("only its owner should read the key: %v", info.Mode().Perm())
		}
	}
	for _, want := range []string{
		"key and certificate: " + keyPath + "\n",
		"certificate alone:   " + certPath + "\n",
		fmt.Sprintf("certificate SHA-256: %x\n", sha256.Sum256(cert)),
		"gh secret set RELEASE_KEY < release.pem\n",
	} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("missing %q in:\n%s", want, r.stdout)
		}
	}

	again := runIn(dir, "key", "new", "release.pem")
	if again.code != ExitFailure || !strings.Contains(again.stderr, "never replaces a key") {
		t.Fatalf("a second key over the first: exit %d, %s", again.code, again.stderr)
	}
	if after, _ := os.ReadFile(keyPath); !bytes.Equal(after, data) {
		t.Error("the first key changed")
	}
}

func TestKeyNewTakesAName(t *testing.T) {
	dir := t.TempDir()
	if r := runIn(dir, "key", "new", "test.pem", "--name", "Meridian test key"); r.code != ExitOK {
		t.Fatalf("exit %d: %s", r.code, r.stderr)
	}
	data, _ := os.ReadFile(filepath.Join(dir, "test.pem"))
	signer, err := authenticode.ParseSigner(data)
	if err != nil || signer.Cert.Subject.CommonName != "Meridian test key" {
		t.Fatalf("%v, %v", signer.Cert, err)
	}
}

func TestKeyNewRefusesWhatItCannotDo(t *testing.T) {
	dir := t.TempDir()
	for _, args := range [][]string{
		{"key"}, {"key", "new"}, {"key", "old", "a.pem"}, {"key", "new", "a.pem", "b.pem"},
		{"key", "new", "a.cer"}, {"key", "new", "--colour", "a.pem"},
	} {
		if r := runIn(dir, args...); r.code != ExitUsage {
			t.Errorf("%q: exit %d, %s", args, r.code, r.stderr)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "kept.cer"), []byte("someone's"), 0o644); err != nil {
		t.Fatal(err)
	}
	if r := runIn(dir, "key", "new", "kept.pem"); r.code != ExitFailure || !strings.Contains(r.stderr, "kept.cer is there already") {
		t.Errorf("a certificate in the way: exit %d, %s", r.code, r.stderr)
	}
	entries, _ := os.ReadDir(dir)
	if len(entries) != 1 {
		t.Errorf("nothing should be written when a file is in the way, found %d files", len(entries))
	}
}

// The release certificate in the repository is the one SECURITY.md tells
// people to check, so it cannot be swapped without the document noticing.
func TestTheReleaseCertificateIsTheOneSecurityNames(t *testing.T) {
	root := realRoot(t)
	raw, err := os.ReadFile(filepath.Join(root, "keys", "release.cer"))
	if err != nil {
		t.Fatal(err)
	}
	cert, err := x509.ParseCertificate(raw)
	if err != nil {
		t.Fatal(err)
	}
	if cert.Subject.CommonName != "Meridian release key" || len(cert.ExtKeyUsage) != 1 ||
		cert.ExtKeyUsage[0] != x509.ExtKeyUsageCodeSigning {
		t.Errorf("the release certificate is for %q, %v", cert.Subject.CommonName, cert.ExtKeyUsage)
	}
	security, err := os.ReadFile(filepath.Join(root, "SECURITY.md"))
	if err != nil {
		t.Fatal(err)
	}
	if sum := fmt.Sprintf("%x", sha256.Sum256(raw)); !strings.Contains(string(security), "`"+sum+"`") {
		t.Errorf("SECURITY.md should name the release certificate's SHA-256 %s", sum)
	}
}

func TestAFileThatCannotBeWrittenSaysSo(t *testing.T) {
	path := filepath.Join(t.TempDir(), "read-only")
	if err := os.WriteFile(path, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := finish(f, []byte("key")); err == nil {
		t.Error("writing to a file opened for reading should fail")
	}
	if err := f.Close(); err == nil {
		t.Error("finish should have closed the file")
	}
	if err := writeNew(path, []byte("key"), 0o600); err == nil {
		t.Error("writeNew should not write over a file that is there")
	}
}
