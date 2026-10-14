package authenticode

import (
	"bytes"
	"crypto"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/asn1"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"unicode/utf16"
)

// obsolete is the file name Authenticode signatures have carried in place
// of a real one since the field was given up.
const obsolete = "<<<Obsolete>>>"

const (
	certificateRevision = 0x0200
	pkcsSignedData      = 0x0002
)

// Signer is a key with the certificate that names it.
type Signer struct {
	Key  *rsa.PrivateKey
	Cert *x509.Certificate
	// Rand is where the signature's randomness comes from; nil means
	// crypto/rand. PKCS #1 v1.5 signatures need none, so this only matters
	// for keys that blind their operations.
	Rand io.Reader
}

// Sign returns the image with an Authenticode signature by s. A signature
// the image had already is replaced, and the image is padded to a multiple
// of 8 bytes first, as the certificate table has to start there.
func Sign(image []byte, s Signer) ([]byte, error) {
	if s.Key == nil || s.Cert == nil {
		return nil, errors.New("signing needs a key and its certificate")
	}
	if pub, ok := s.Cert.PublicKey.(*rsa.PublicKey); !ok || !pub.Equal(&s.Key.PublicKey) {
		return nil, errors.New("the certificate does not belong to the key")
	}
	l, err := parse(image)
	if err != nil {
		return nil, err
	}
	out := pad8(bytes.Clone(image[:len(image)-l.certs.size]))
	l.certs = span{}
	signature, err := s.signedData(l.digest(out))
	if err != nil {
		return nil, err
	}
	entry := binary.LittleEndian.AppendUint32(nil, uint32(8+len(signature)))
	entry = binary.LittleEndian.AppendUint16(entry, certificateRevision)
	entry = binary.LittleEndian.AppendUint16(entry, pkcsSignedData)
	entry = pad8(append(entry, signature...))
	binary.LittleEndian.PutUint32(out[l.certDirAt:], uint32(len(out)))
	binary.LittleEndian.PutUint32(out[l.certDirAt+4:], uint32(len(entry)))
	return append(out, entry...), nil
}

// pad8 fills b with zeros to a multiple of 8 bytes, where certificate
// table entries have to start.
func pad8(b []byte) []byte {
	return append(b, make([]byte, (8-len(b)%8)%8)...)
}

// indirectData is the SpcIndirectDataContent: what is signed, a PE image,
// and its digest.
func indirectData(digest [sha256.Size]byte) []byte {
	name := utf16.Encode([]rune(obsolete))
	bmp := make([]byte, 0, 2*len(name))
	for _, u := range name {
		bmp = binary.BigEndian.AppendUint16(bmp, u)
	}
	file := tlv(explicit0, tlv(explicit2, tlv(implicitBMP0, bmp)))
	image := seq(tlv(tagBitString, []byte{0}), file)
	return seq(
		seq(oid(oidSpcPEImageData), image),
		seq(sha256Algorithm(), octets(digest[:])),
	)
}

func sha256Algorithm() []byte { return seq(oid(oidSHA256), tlv(tagNull)) }

func attribute(id asn1.ObjectIdentifier, value []byte) []byte {
	return seq(oid(id), tlv(tagSet, value))
}

// sortedSet encodes the elements as the contents of a DER SET OF, which
// sorts them by their encodings.
func sortedSet(elements ...[]byte) []byte {
	sorted := append([][]byte(nil), elements...)
	for i := 1; i < len(sorted); i++ {
		for j := i; j > 0 && bytes.Compare(sorted[j], sorted[j-1]) < 0; j-- {
			sorted[j], sorted[j-1] = sorted[j-1], sorted[j]
		}
	}
	return bytes.Join(sorted, nil)
}

// signedData is the PKCS #7 ContentInfo holding the signature over the
// image's digest.
func (s Signer) signedData(digest [sha256.Size]byte) ([]byte, error) {
	content := indirectData(digest)
	inner, err := contents(content)
	if err != nil {
		return nil, err
	}
	// PKCS #7 hashes the content without its own tag and length.
	messageDigest := sha256.Sum256(inner)
	attributes := sortedSet(
		attribute(oidContentType, oid(oidSpcIndirectData)),
		attribute(oidSpcSpOpusInfo, seq()),
		attribute(oidSpcStatementType, seq(oid(oidIndividualSign))),
		attribute(oidMessageDigest, octets(messageDigest[:])),
	)
	// The signature covers the attributes encoded as a SET, though they
	// are stored with the [0] tag.
	toSign := sha256.Sum256(tlv(tagSet, attributes))
	random := s.Rand
	if random == nil {
		random = rand.Reader
	}
	sig, err := rsa.SignPKCS1v15(random, s.Key, crypto.SHA256, toSign[:])
	if err != nil {
		return nil, fmt.Errorf("sign: %w", err)
	}
	serial, err := asn1.Marshal(s.Cert.SerialNumber)
	if err != nil {
		return nil, fmt.Errorf("the certificate's serial number: %w", err)
	}
	signer := seq(
		small(1),
		seq(s.Cert.RawIssuer, serial),
		sha256Algorithm(),
		tlv(implicit0, attributes),
		seq(oid(oidRSA), tlv(tagNull)),
		octets(sig),
	)
	signed := seq(
		small(1),
		tlv(tagSet, sha256Algorithm()),
		seq(oid(oidSpcIndirectData), tlv(explicit0, content)),
		tlv(implicit0, s.Cert.Raw),
		tlv(tagSet, signer),
	)
	return seq(oid(oidSignedData), tlv(explicit0, signed)), nil
}
