package secureboot

import (
	"bytes"
	"crypto/rand"
	"encoding/binary"
	"encoding/pem"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/gpt"
)

var (
	signerOnce sync.Once
	signer     authenticode.Signer
	signerErr  error
	owner      = mustGUID("0E6F5E4C-2A33-4B8C-9C0D-5D2E8E1A7F31")
	when       = time.Date(2026, 10, 14, 21, 30, 5, 123, time.UTC)
)

func testSigner(t *testing.T) authenticode.Signer {
	t.Helper()
	signerOnce.Do(func() {
		signer, signerErr = authenticode.NewSigner("Meridian test key", time.Unix(1_760_000_000, 0), rand.Reader)
	})
	if signerErr != nil {
		t.Fatal(signerErr)
	}
	return signer
}

func TestTheGUIDsAreStoredAsUEFIStoresThem(t *testing.T) {
	cases := map[string]struct {
		got  gpt.GUID
		want []byte
	}{
		"global":   {GlobalVariable, []byte{0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c}},
		"security": {ImageSecurityDatabase, []byte{0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f}},
		"x509":     {certX509, []byte{0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72}},
		"pkcs7":    {certTypePKCS7, []byte{0x9d, 0xd2, 0xaf, 0x4a, 0xdf, 0x68, 0xee, 0x49, 0x8a, 0xa9, 0x34, 0x7d, 0x37, 0x56, 0x65, 0xa7}},
	}
	for name, c := range cases {
		if !bytes.Equal(c.got[:], c.want) {
			t.Errorf("%s: % x", name, c.got[:])
		}
	}
}

func TestASignatureListHoldsOneCertificate(t *testing.T) {
	cert := []byte("a certificate in DER")
	got := SignatureList(owner, cert)
	u32 := func(at int) uint32 { return binary.LittleEndian.Uint32(got[at:]) }
	if !bytes.Equal(got[:16], certX509[:]) || u32(16) != uint32(len(got)) || u32(20) != 0 || u32(24) != 16+uint32(len(cert)) {
		t.Fatalf("header % x", got[:28])
	}
	if !bytes.Equal(got[28:44], owner[:]) || !bytes.Equal(got[44:], cert) || len(got) != 28+16+len(cert) {
		t.Fatalf("entry % x", got[28:])
	}
}

func TestTimesAreUTCToTheSecond(t *testing.T) {
	zurich := time.FixedZone("CEST", 2*60*60)
	got, err := Time(time.Date(2026, 10, 14, 23, 30, 5, 999, zurich))
	if err != nil {
		t.Fatal(err)
	}
	want := []byte{0xea, 0x07, 10, 14, 21, 30, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0}
	if !bytes.Equal(got, want) {
		t.Fatalf("got % x", got)
	}
	for _, year := range []int{1900, 9999} {
		if _, err := Time(time.Date(year, 6, 1, 0, 0, 0, 0, time.UTC)); err != nil {
			t.Errorf("%d: %v", year, err)
		}
	}
	for _, year := range []int{1899, 10000} {
		if _, err := Time(time.Date(year, 6, 1, 0, 0, 0, 0, time.UTC)); err == nil || !strings.Contains(err.Error(), "1900 to 9999") {
			t.Errorf("%d: %v", year, err)
		}
	}
}

// signedPart is what an authenticated write of v covers, as the firmware
// puts it together.
func signedPart(v Variable, stamp, data []byte) []byte {
	var out []byte
	for _, c := range v.Name {
		out = binary.LittleEndian.AppendUint16(out, uint16(c))
	}
	out = append(out, v.Vendor[:]...)
	// Non-volatile, boot and runtime access, time based writes.
	out = binary.LittleEndian.AppendUint32(out, 0x27)
	out = append(out, stamp...)
	return append(out, data...)
}

func TestAnAuthenticatedWriteCarriesItsTimeSignatureAndData(t *testing.T) {
	s := testSigner(t)
	data := SignatureList(owner, s.Cert.Raw)
	v := Variable{"KEK", GlobalVariable}
	got, err := Authenticated(v, when, data, s)
	if err != nil {
		t.Fatal(err)
	}
	stamp, _ := Time(when)
	length := int(binary.LittleEndian.Uint32(got[16:]))
	if !bytes.Equal(got[:16], stamp) || binary.LittleEndian.Uint16(got[20:]) != 0x0200 || binary.LittleEndian.Uint16(got[22:]) != 0x0ef1 ||
		!bytes.Equal(got[24:40], certTypePKCS7[:]) {
		t.Fatalf("header % x", got[:40])
	}
	if len(got) != 16+length+len(data) || !bytes.Equal(got[16+length:], data) {
		t.Fatalf("%d bytes, the certificate %d, the data %d", len(got), length, len(data))
	}
	signature, err := s.SignDetached(signedPart(v, stamp, data))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got[40:16+length], signature) {
		t.Error("the signature covers the name, vendor, attributes, time and data")
	}
	if _, err := Authenticated(v, time.Date(1850, 1, 1, 0, 0, 0, 0, time.UTC), data, s); err == nil {
		t.Error("a time UEFI cannot hold")
	}
	if _, err := Authenticated(v, when, data, authenticode.Signer{Cert: s.Cert}); err == nil {
		t.Error("no key")
	}
}

// OpenSSL checks each signature against what the firmware will put
// together, and refuses it for another variable name.
func TestOpenSSLVerifiesTheEnrollment(t *testing.T) {
	openssl, err := exec.LookPath("openssl")
	if err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatal("openssl is not installed and SEXTANT_REQUIRE_TOOLS=1")
		}
		t.Skip("openssl is not installed")
	}
	s := testSigner(t)
	writes, err := Enroll(s, owner, when)
	if err != nil {
		t.Fatal(err)
	}
	if len(writes) != 3 || Databases[2].Name != "PK" {
		t.Fatalf("%d writes, the last for %s", len(writes), Databases[2].Name)
	}
	dir := t.TempDir()
	file := func(name string, data []byte) string {
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return path
	}
	ca := file("cert.pem", pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Cert.Raw}))
	list := SignatureList(owner, s.Cert.Raw)
	stamp, _ := Time(when)
	for i, v := range Databases {
		w := writes[i]
		length := int(binary.LittleEndian.Uint32(w[16:]))
		signature := w[40 : 16+length]
		if !bytes.Equal(w[16+length:], list) {
			t.Fatalf("%s: the data is the certificate's list", v.Name)
		}
		// OpenSSL wants the SignedData inside a ContentInfo.
		wrapped := append([]byte{0x30, 0x82, 0, 0, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02, 0xa0, 0x82, 0, 0}, signature...)
		binary.BigEndian.PutUint16(wrapped[2:], uint16(len(wrapped)-4))
		binary.BigEndian.PutUint16(wrapped[17:], uint16(len(signature)))
		sig := file(v.Name+".p7", wrapped)
		verify := func(v Variable) (string, error) {
			out, err := exec.Command(openssl, "smime", "-verify", "-inform", "DER", "-in", sig, "-binary",
				"-content", file("content", signedPart(v, stamp, list)), "-CAfile", ca, "-purpose", "any",
				"-out", filepath.Join(dir, "out")).CombinedOutput()
			return string(out), err
		}
		if out, err := verify(v); err != nil || !strings.Contains(out, "Verification successful") {
			t.Fatalf("%s: %v\n%s", v.Name, err, out)
		}
		if out, err := verify(Variable{"dbx", v.Vendor}); err == nil {
			t.Fatalf("%s: OpenSSL accepted it for dbx:\n%s", v.Name, out)
		}
	}
	if _, err := Enroll(authenticode.Signer{}, owner, when); err == nil {
		t.Error("enrolling without a certificate")
	}
}
