package authenticode

import (
	"bytes"
	"crypto/rand"
	"encoding/asn1"
	"encoding/pem"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// OpenSSL checks the detached signature against the content and the
// certificate, once wrapped in the ContentInfo it expects around it.
func TestOpenSSLVerifiesADetachedSignature(t *testing.T) {
	openssl := requireTool(t, "openssl")
	s := testSigner(t)
	content := []byte("PK\x00a variable's name, vendor, attributes, time and data")
	sd, err := s.SignDetached(content)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	write := func(name string, data []byte) string {
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return path
	}
	sig := write("sig.der", seq(oid(oidSignedData), tlv(explicit0, sd)))
	ca := write("cert.pem", pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Cert.Raw}))
	verify := func(content []byte) (string, error) {
		out, err := exec.Command(openssl, "smime", "-verify", "-inform", "DER", "-in", sig, "-binary",
			"-content", write("content.bin", content), "-CAfile", ca, "-purpose", "any",
			"-out", filepath.Join(dir, "out.bin")).CombinedOutput()
		return string(out), err
	}
	if out, err := verify(content); err != nil || !strings.Contains(out, "Verification successful") {
		t.Fatalf("openssl: %v\n%s", err, out)
	}
	tampered := bytes.Clone(content)
	tampered[0] ^= 1
	if out, err := verify(tampered); err == nil {
		t.Fatalf("openssl accepted changed content:\n%s", out)
	}
}

// Firmware finds the digest algorithm at a fixed place: the SignedData
// starts with a two byte length, and the SHA-256 identifier follows 13
// bytes in.
func TestADetachedSignatureHasTheLayoutFirmwareReads(t *testing.T) {
	sd, err := testSigner(t).SignDetached([]byte("data"))
	if err != nil {
		t.Fatal(err)
	}
	sha256OID := []byte{0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01}
	if sd[0] != 0x30 || sd[1] != 0x82 || !bytes.Equal(sd[13:13+len(sha256OID)], sha256OID) {
		t.Fatalf("starts % x", sd[:24])
	}
	var parsed struct {
		Version    int
		Digests    asn1.RawValue `asn1:"set"`
		Content    struct{ Type asn1.ObjectIdentifier }
		Cert       asn1.RawValue `asn1:"tag:0"`
		SignerInfo []struct {
			Version   int
			Issuer    asn1.RawValue
			Digest    asn1.RawValue
			Algorithm struct {
				ID     asn1.ObjectIdentifier
				Params asn1.RawValue
			}
			Signature []byte
		} `asn1:"set"`
	}
	rest, err := asn1.Unmarshal(sd, &parsed)
	if err != nil || len(rest) != 0 {
		t.Fatalf("parse: %v, %d bytes left", err, len(rest))
	}
	if parsed.Version != 1 || !parsed.Content.Type.Equal(oidData) || len(parsed.SignerInfo) != 1 {
		t.Fatalf("got %+v", parsed)
	}
	if !bytes.Equal(parsed.Cert.Bytes, testSigner(t).Cert.Raw) {
		t.Error("the certificate is carried whole")
	}
	si := parsed.SignerInfo[0]
	if si.Version != 1 || !si.Algorithm.ID.Equal(oidRSA) || len(si.Signature) != 256 {
		t.Errorf("the signer: version %d, algorithm %v, %d byte signature, without signed attributes", si.Version, si.Algorithm.ID, len(si.Signature))
	}
}

func TestDetachedSignaturesFollowTheContent(t *testing.T) {
	s := testSigner(t)
	a, errA := s.SignDetached([]byte("one"))
	again, errAgain := s.SignDetached([]byte("one"))
	b, errB := s.SignDetached([]byte("two"))
	if errA != nil || errAgain != nil || errB != nil {
		t.Fatal(errA, errAgain, errB)
	}
	if !bytes.Equal(a, again) {
		t.Error("the same content signs the same")
	}
	if bytes.Equal(a, b) || len(a) != len(b) {
		t.Error("other content, another signature of the same size")
	}
}

func TestDetachedSigningNeedsAMatchingKey(t *testing.T) {
	s := testSigner(t)
	if _, err := (Signer{Cert: s.Cert}).SignDetached([]byte("x")); err == nil {
		t.Error("no key")
	}
	other, err := NewSigner("another key", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := (Signer{Key: other.Key, Cert: s.Cert}).SignDetached([]byte("x")); err == nil || !strings.Contains(err.Error(), "does not belong") {
		t.Errorf("a key with someone else's certificate: %v", err)
	}
}
