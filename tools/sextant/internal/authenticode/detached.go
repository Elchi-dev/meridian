package authenticode

import (
	"crypto/sha256"
	"encoding/asn1"
	"fmt"
)

var oidData = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 7, 1}

// SignDetached returns a PKCS #7 SignedData by s over content, which it
// does not carry, the form UEFI takes for authenticated variables (UEFI
// 2.10, section 8.2.2): the SignedData without a ContentInfo around it,
// and without signed attributes, so the signature is over the content's
// digest itself.
func (s Signer) SignDetached(content []byte) ([]byte, error) {
	if err := s.usable(); err != nil {
		return nil, err
	}
	sig, err := s.sign(sha256.Sum256(content))
	if err != nil {
		return nil, err
	}
	serial, err := asn1.Marshal(s.Cert.SerialNumber)
	if err != nil {
		return nil, fmt.Errorf("the certificate's serial number: %w", err)
	}
	signer := seq(
		small(1),
		seq(s.Cert.RawIssuer, serial),
		sha256Algorithm(),
		seq(oid(oidRSA), tlv(tagNull)),
		octets(sig),
	)
	return seq(
		small(1),
		tlv(tagSet, sha256Algorithm()),
		seq(oid(oidData)),
		tlv(implicit0, s.Cert.Raw),
		tlv(tagSet, signer),
	), nil
}
