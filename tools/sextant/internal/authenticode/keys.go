package authenticode

import (
	"crypto/rsa"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"time"
)

// KeyBits is the size of the keys sextant makes. UEFI firmware checks
// RSA-2048 signatures; larger keys are not supported everywhere.
const KeyBits = 2048

// NewSigner makes a key and a certificate for it, signed by itself, valid
// from now for 30 years. Firmware does not look at the dates; other tools
// that read the certificate do.
func NewSigner(name string, now time.Time, random io.Reader) (Signer, error) {
	key, err := rsa.GenerateKey(random, KeyBits)
	if err != nil {
		return Signer{}, fmt.Errorf("make a key: %w", err)
	}
	serial, err := randomSerial(random)
	if err != nil {
		return Signer{}, err
	}
	template := &x509.Certificate{
		SerialNumber:          serial,
		Subject:               pkix.Name{CommonName: name},
		NotBefore:             now.Add(-time.Hour),
		NotAfter:              now.AddDate(30, 0, 0),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageCodeSigning},
		BasicConstraintsValid: true,
		IsCA:                  true,
	}
	raw, err := x509.CreateCertificate(random, template, template, &key.PublicKey, key)
	if err != nil {
		return Signer{}, fmt.Errorf("make a certificate: %w", err)
	}
	cert, err := x509.ParseCertificate(raw)
	if err != nil {
		return Signer{}, err
	}
	return Signer{Key: key, Cert: cert}, nil
}

// randomSerial is a positive serial number of 127 random bits.
func randomSerial(random io.Reader) (*big.Int, error) {
	b := make([]byte, 16)
	if _, err := io.ReadFull(random, b); err != nil {
		return nil, fmt.Errorf("make a serial number: %w", err)
	}
	b[0] &= 0x7f
	return new(big.Int).SetBytes(b), nil
}

// PEM is the key and the certificate as PEM blocks, the key first.
func (s Signer) PEM() []byte {
	out := pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: mustPKCS8(s.Key)})
	return append(out, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Cert.Raw})...)
}

func mustPKCS8(key *rsa.PrivateKey) []byte {
	b, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		panic(fmt.Sprintf("an RSA key cannot be written as PKCS #8: %v", err))
	}
	return b
}

// ParseSigner reads what PEM wrote: an RSA key and its certificate.
func ParseSigner(data []byte) (Signer, error) {
	var s Signer
	for {
		var block *pem.Block
		block, data = pem.Decode(data)
		if block == nil {
			break
		}
		switch block.Type {
		case "PRIVATE KEY":
			key, err := x509.ParsePKCS8PrivateKey(block.Bytes)
			if err != nil {
				return Signer{}, fmt.Errorf("the key: %w", err)
			}
			rsaKey, ok := key.(*rsa.PrivateKey)
			if !ok {
				return Signer{}, errors.New("the key is not an RSA key")
			}
			s.Key = rsaKey
		case "CERTIFICATE":
			cert, err := x509.ParseCertificate(block.Bytes)
			if err != nil {
				return Signer{}, fmt.Errorf("the certificate: %w", err)
			}
			s.Cert = cert
		}
	}
	if s.Key == nil || s.Cert == nil {
		return Signer{}, errors.New("a signer needs a PRIVATE KEY and a CERTIFICATE block")
	}
	return s, nil
}
