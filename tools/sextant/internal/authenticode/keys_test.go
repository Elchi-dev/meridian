package authenticode

import (
	"bytes"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"encoding/pem"
	"strings"
	"testing"
	"time"
)

func TestANewKeyHasACertificateForSigningCode(t *testing.T) {
	s := testSigner(t)
	c := s.Cert
	if c.Subject.CommonName != "Meridian test key" || c.Issuer.CommonName != "Meridian test key" {
		t.Errorf("subject %q, issuer %q", c.Subject, c.Issuer)
	}
	if s.Key.N.BitLen() != KeyBits || KeyBits != 2048 {
		t.Errorf("a key of %d bits", s.Key.N.BitLen())
	}
	if !c.IsCA || len(c.ExtKeyUsage) != 1 || c.ExtKeyUsage[0] != x509.ExtKeyUsageCodeSigning {
		t.Errorf("CA %v, uses %v", c.IsCA, c.ExtKeyUsage)
	}
	if c.KeyUsage&x509.KeyUsageDigitalSignature == 0 {
		t.Error("the key should be allowed to sign")
	}
	made := time.Unix(1_760_000_000, 0)
	if !c.NotBefore.Before(made) || c.NotAfter.Before(made.AddDate(29, 11, 0)) || c.NotAfter.After(made.AddDate(30, 0, 1)) {
		t.Errorf("valid from %v to %v", c.NotBefore, c.NotAfter)
	}
	if c.SerialNumber.Sign() <= 0 || c.SerialNumber.BitLen() > 127 {
		t.Errorf("serial %v", c.SerialNumber)
	}
	if err := c.CheckSignatureFrom(c); err != nil {
		t.Errorf("the certificate should be signed by its own key: %v", err)
	}
}

func TestSignersSurviveAWriteAndARead(t *testing.T) {
	s := testSigner(t)
	data := s.PEM()
	if !bytes.HasPrefix(data, []byte("-----BEGIN PRIVATE KEY-----")) || !bytes.Contains(data, []byte("-----BEGIN CERTIFICATE-----")) {
		t.Fatalf("PEM:\n%s", data)
	}
	back, err := ParseSigner(data)
	if err != nil {
		t.Fatal(err)
	}
	if !back.Key.Equal(s.Key) || !back.Cert.Equal(s.Cert) {
		t.Fatal("the signer read back differs")
	}
	a, _ := Sign(sampleImage(), s)
	b, _ := Sign(sampleImage(), back)
	if !bytes.Equal(a, b) {
		t.Fatal("the signer read back signs differently")
	}
}

func TestBrokenSignerFilesAreRefused(t *testing.T) {
	s := testSigner(t)
	keyOnly, _, _ := strings.Cut(string(s.PEM()), "-----BEGIN CERTIFICATE-----")
	certOnly := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Cert.Raw})
	ec, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	ecDER, err := x509.MarshalPKCS8PrivateKey(ec)
	if err != nil {
		t.Fatal(err)
	}
	cases := map[string]string{
		"nothing":              "",
		"only the key":         keyOnly,
		"only a certificate":   string(certOnly),
		"a broken key":         string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: []byte{1, 2}})) + string(certOnly),
		"a broken certificate": keyOnly + string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: []byte{3}})),
		"an elliptic key":      string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: ecDER})) + string(certOnly),
	}
	for name, data := range cases {
		if _, err := ParseSigner([]byte(data)); err == nil {
			t.Errorf("%s: no error", name)
		}
	}
	other := "-----BEGIN NOTES-----\naGk=\n-----END NOTES-----\n" + string(s.PEM())
	if _, err := ParseSigner([]byte(other)); err != nil {
		t.Errorf("other blocks are skipped: %v", err)
	}
}

func TestMakingAKeyNeedsRandomness(t *testing.T) {
	if _, err := randomSerial(strings.NewReader("short")); err == nil {
		t.Fatal("five random bytes are not enough for a serial number")
	}
	serial, err := randomSerial(bytes.NewReader(bytes.Repeat([]byte{0xff}, 16)))
	if err != nil {
		t.Fatal(err)
	}
	if serial.BitLen() != 127 {
		t.Fatalf("the top bit is cleared, so the serial stays positive: %d bits", serial.BitLen())
	}
}
