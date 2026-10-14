package authenticode

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"debug/pe"
	"encoding/asn1"
	"encoding/binary"
	"encoding/pem"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

var (
	signerOnce sync.Once
	signer     Signer
	signerErr  error
)

// testSigner is made once: making an RSA key takes a while.
func testSigner(t *testing.T) Signer {
	t.Helper()
	signerOnce.Do(func() {
		signer, signerErr = NewSigner("Meridian test key", time.Unix(1_760_000_000, 0), rand.Reader)
	})
	if signerErr != nil {
		t.Fatal(signerErr)
	}
	return signer
}

func requireTool(t *testing.T, name string) string {
	t.Helper()
	path, err := exec.LookPath(name)
	if err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatalf("%s is not installed and SEXTANT_REQUIRE_TOOLS=1", name)
		}
		t.Skipf("%s is not installed, skipping the cross-check", name)
	}
	return path
}

// readCertificates returns the PKCS #7 signature from a signed image,
// read through Go's PE reader.
func readCertificates(t *testing.T, signed []byte) []byte {
	t.Helper()
	f, err := pe.NewFile(bytes.NewReader(signed))
	if err != nil {
		t.Fatal(err)
	}
	var dir pe.DataDirectory
	switch o := f.OptionalHeader.(type) {
	case *pe.OptionalHeader64:
		dir = o.DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_SECURITY]
	case *pe.OptionalHeader32:
		dir = o.DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_SECURITY]
	}
	if int(dir.VirtualAddress)+int(dir.Size) != len(signed) || dir.VirtualAddress%8 != 0 || dir.Size%8 != 0 {
		t.Fatalf("the certificate table at %#x, %d bytes, in a file of %d", dir.VirtualAddress, dir.Size, len(signed))
	}
	entry := signed[dir.VirtualAddress:]
	length := binary.LittleEndian.Uint32(entry)
	if binary.LittleEndian.Uint16(entry[4:]) != 0x0200 || binary.LittleEndian.Uint16(entry[6:]) != 2 {
		t.Fatalf("WIN_CERTIFICATE header % x", entry[:8])
	}
	if length > dir.Size || length+7 < dir.Size {
		t.Fatalf("dwLength %d for a table of %d", length, dir.Size)
	}
	return entry[8:length]
}

// signedContent walks ContentInfo, SignedData and its content to the
// SpcIndirectDataContent, and returns it whole.
func signedContent(t *testing.T, signature []byte) []byte {
	t.Helper()
	var info struct {
		Type    asn1.ObjectIdentifier
		Content asn1.RawValue `asn1:"explicit,tag:0"`
	}
	if rest, err := asn1.Unmarshal(signature, &info); err != nil || len(rest) != 0 {
		t.Fatalf("ContentInfo: %v, %d bytes after it", err, len(rest))
	}
	if !info.Type.Equal(oidSignedData) {
		t.Fatalf("content type %v", info.Type)
	}
	var signed struct {
		Version    int
		Digests    asn1.RawValue
		Content    asn1.RawValue
		Remainders []asn1.RawValue `asn1:"optional"`
	}
	// An explicitly tagged RawValue holds the tag; its Bytes are the
	// element inside.
	if _, err := asn1.Unmarshal(info.Content.Bytes, &signed); err != nil {
		t.Fatalf("SignedData: %v", err)
	}
	var content struct {
		Type    asn1.ObjectIdentifier
		Content asn1.RawValue `asn1:"explicit,tag:0"`
	}
	if _, err := asn1.Unmarshal(signed.Content.FullBytes, &content); err != nil {
		t.Fatalf("the signed content: %v", err)
	}
	if !content.Type.Equal(oidSpcIndirectData) {
		t.Fatalf("the signed content is %v", content.Type)
	}
	return content.Content.Bytes
}

// embeddedDigest is the image digest inside an SpcIndirectDataContent.
func embeddedDigest(t *testing.T, indirect []byte) []byte {
	t.Helper()
	var spc struct {
		Data   asn1.RawValue
		Digest struct {
			Algorithm struct {
				ID     asn1.ObjectIdentifier
				Params asn1.RawValue `asn1:"optional"`
			}
			Value []byte
		}
	}
	if _, err := asn1.Unmarshal(indirect, &spc); err != nil {
		t.Fatalf("SpcIndirectDataContent: %v", err)
	}
	if !spc.Digest.Algorithm.ID.Equal(oidSHA256) {
		t.Fatalf("digest algorithm %v", spc.Digest.Algorithm.ID)
	}
	return spc.Digest.Value
}

func sampleImage() []byte {
	return buildPE(true, []fakeSection{{0x400, filled(0x200, 1)}, {0x600, filled(0x200, 2)}}, filled(5, 3))
}

func TestASignedImageCarriesItsDigest(t *testing.T) {
	image := sampleImage()
	signed, err := Sign(image, testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	if len(signed)%8 != 0 {
		t.Fatalf("%d bytes, not a multiple of 8", len(signed))
	}
	padded := append(bytes.Clone(image), 0, 0, 0)
	before := bytes.Clone(signed[:len(padded)])
	entry := peAt + 24 + 112 + 4*8
	if binary.LittleEndian.Uint32(before[entry:]) != uint32(len(padded)) {
		t.Fatal("the certificate table's entry should point past the padded image")
	}
	clear(before[entry : entry+8])
	if !bytes.Equal(before, padded) {
		t.Fatal("apart from the certificate table's entry, the image should be the original padded to 8 bytes")
	}
	want, _ := Digest(padded)
	if got, err := Digest(signed); err != nil || got != want {
		t.Fatalf("the signed image's digest %x, %v; want %x", got, err, want)
	}
	if got := embeddedDigest(t, signedContent(t, readCertificates(t, signed))); !bytes.Equal(got, want[:]) {
		t.Fatalf("the signature names digest %x, want %x", got, want)
	}
	if want != referenceDigest(t, signed) {
		t.Fatal("the reference reads a different digest from the signed image")
	}
}

func TestSigningAgainReplacesTheSignature(t *testing.T) {
	once, err := Sign(sampleImage(), testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	twice, err := Sign(once, testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	if len(twice) != len(once) {
		t.Fatalf("signing twice gave %d bytes, once %d", len(twice), len(once))
	}
	// PKCS #1 v1.5 signatures are deterministic, so the same key over the
	// same image gives the same bytes.
	if !bytes.Equal(twice, once) {
		t.Fatal("signing a signed image again should give the same result")
	}
}

func TestSigningNeedsAMatchingKeyAndAnImage(t *testing.T) {
	s := testSigner(t)
	if _, err := Sign(sampleImage(), Signer{Cert: s.Cert}); err == nil {
		t.Error("no key")
	}
	if _, err := Sign(sampleImage(), Signer{Key: s.Key}); err == nil {
		t.Error("no certificate")
	}
	other, err := NewSigner("another key", time.Unix(1_760_000_000, 0), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Sign(sampleImage(), Signer{Key: other.Key, Cert: s.Cert}); err == nil || !strings.Contains(err.Error(), "does not belong") {
		t.Errorf("a key with someone else's certificate: %v", err)
	}
	if _, err := Sign([]byte("not an image"), s); err == nil {
		t.Error("not a PE file")
	}
}

// OpenSSL reads the signature on its own: the content's digest in the
// signed attributes, the RSA signature over them and the certificate.
func TestOpenSSLVerifiesTheSignature(t *testing.T) {
	openssl := requireTool(t, "openssl")
	s := testSigner(t)
	signed, err := Sign(sampleImage(), s)
	if err != nil {
		t.Fatal(err)
	}
	signature := readCertificates(t, signed)
	inner, err := contents(signedContent(t, signature))
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
	sig := write("sig.der", signature)
	ca := write("cert.pem", pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Cert.Raw}))
	verify := func(content []byte) (string, error) {
		out, err := exec.Command(openssl, "smime", "-verify", "-inform", "DER", "-in", sig, "-binary",
			"-content", write("content.bin", content), "-CAfile", ca, "-purpose", "any",
			"-out", filepath.Join(dir, "out.bin")).CombinedOutput()
		return string(out), err
	}
	if out, err := verify(inner); err != nil || !strings.Contains(out, "Verification successful") {
		t.Fatalf("openssl: %v\n%s", err, out)
	}
	tampered := bytes.Clone(inner)
	tampered[len(tampered)-1] ^= 1
	if out, err := verify(tampered); err == nil {
		t.Fatalf("openssl accepted content with a changed digest:\n%s", out)
	}
	printed, err := exec.Command(openssl, "pkcs7", "-inform", "DER", "-in", sig, "-print_certs", "-noout").CombinedOutput()
	// OpenSSL 3.0 writes "CN = name", later versions "CN=name".
	if err != nil || !strings.Contains(string(printed), "subject=") || !strings.Contains(string(printed), "Meridian test key") {
		t.Fatalf("openssl pkcs7: %v\n%s", err, printed)
	}
}

func TestTheDigestCoversTheImage(t *testing.T) {
	image := sampleImage()
	signed, err := Sign(image, testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	changed := bytes.Clone(image)
	changed[0x500] ^= 1
	other, err := Sign(changed, testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	a := embeddedDigest(t, signedContent(t, readCertificates(t, signed)))
	b := embeddedDigest(t, signedContent(t, readCertificates(t, other)))
	if bytes.Equal(a, b) {
		t.Fatal("a changed section should change the signed digest")
	}
	if sha := sha256.Sum256(nil); bytes.Equal(a, sha[:]) {
		t.Fatal("the digest of nothing")
	}
}

func TestPaddingReachesTheNextMultipleOfEight(t *testing.T) {
	for n := range 18 {
		got := pad8(bytes.Repeat([]byte{9}, n))
		want := (n + 7) / 8 * 8
		if len(got) != want || !bytes.Equal(got[:n], bytes.Repeat([]byte{9}, n)) || bytes.ContainsRune(got[n:], 9) {
			t.Errorf("%d bytes padded to %d, want %d", n, len(got), want)
		}
	}
}

func TestSetsAreSortedByTheirEncoding(t *testing.T) {
	got := sortedSet([]byte{2, 1}, []byte{1, 9}, []byte{2, 0}, []byte{1, 2, 3})
	if want := []byte{1, 2, 3, 1, 9, 2, 0, 2, 1}; !bytes.Equal(got, want) {
		t.Fatalf("% x, want % x", got, want)
	}
	if len(sortedSet()) != 0 {
		t.Fatal("an empty set has no contents")
	}
}

// signedAttributes are the attributes in the signature's SignerInfo, in
// their order, each as its object identifier, its value and its whole
// encoding.
func signedAttributes(t *testing.T, signature []byte) ([]asn1.ObjectIdentifier, [][]byte, [][]byte) {
	t.Helper()
	var info struct {
		Type    asn1.ObjectIdentifier
		Content asn1.RawValue `asn1:"explicit,tag:0"`
	}
	if _, err := asn1.Unmarshal(signature, &info); err != nil {
		t.Fatal(err)
	}
	var signed struct {
		Version      int
		Digests      asn1.RawValue
		Content      asn1.RawValue
		Certificates asn1.RawValue `asn1:"tag:0"`
		Signers      []struct {
			Version    int
			Issuer     asn1.RawValue
			Digest     asn1.RawValue
			Attributes []asn1.RawValue `asn1:"tag:0"`
			Algorithm  asn1.RawValue
			Signature  []byte
		} `asn1:"set"`
	}
	if _, err := asn1.Unmarshal(info.Content.Bytes, &signed); err != nil {
		t.Fatalf("SignedData: %v", err)
	}
	if len(signed.Signers) != 1 {
		t.Fatalf("%d signers", len(signed.Signers))
	}
	var ids []asn1.ObjectIdentifier
	var values, whole [][]byte
	for _, raw := range signed.Signers[0].Attributes {
		var a struct {
			Type  asn1.ObjectIdentifier
			Value asn1.RawValue `asn1:"set"`
		}
		if _, err := asn1.Unmarshal(raw.FullBytes, &a); err != nil {
			t.Fatal(err)
		}
		ids = append(ids, a.Type)
		values = append(values, a.Value.Bytes)
		whole = append(whole, raw.FullBytes)
	}
	return ids, values, whole
}

func TestTheSignedAttributesNameTheContentAndItsDigest(t *testing.T) {
	signed, err := Sign(sampleImage(), testSigner(t))
	if err != nil {
		t.Fatal(err)
	}
	signature := readCertificates(t, signed)
	ids, values, whole := signedAttributes(t, signature)
	inner, err := contents(signedContent(t, signature))
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(inner)
	found := 0
	for i, id := range ids {
		switch {
		case id.Equal(oidContentType):
			found++
			if !bytes.Equal(values[i], oid(oidSpcIndirectData)) {
				t.Errorf("content type % x", values[i])
			}
		case id.Equal(oidMessageDigest):
			found++
			if !bytes.Equal(values[i], octets(digest[:])) {
				t.Errorf("message digest % x, want the digest of the content", values[i])
			}
		}
	}
	if found != 2 || len(ids) != 4 {
		t.Fatalf("attributes %v", ids)
	}
	// DER wants a SET OF in the order of its elements' encodings.
	for i := 1; i < len(whole); i++ {
		if bytes.Compare(whole[i-1], whole[i]) > 0 {
			t.Errorf("attribute %v sorts after %v", ids[i-1], ids[i])
		}
	}
}

// The image data is always the same few bytes: no flags, and the file name
// every Authenticode signer writes, "<<<Obsolete>>>" in UTF-16 behind the
// tags [0], [2] and an implicit [0].
func TestTheImageDataIsTheUsualOne(t *testing.T) {
	want := []byte{0x30, 0x25, 0x03, 0x01, 0x00, 0xa0, 0x20, 0xa2, 0x1e, 0x80, 0x1c}
	for _, c := range "<<<Obsolete>>>" {
		want = append(want, 0, byte(c))
	}
	got := indirectData([sha256.Size]byte{})
	if !bytes.Contains(got, want) {
		t.Fatalf("% x\ndoes not hold\n% x", got, want)
	}
}
