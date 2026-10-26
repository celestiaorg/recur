package chatgpt

import (
	"context"
	"crypto"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"time"
)

type identityClaims struct {
	Subject     string          `json:"sub"`
	Email       string          `json:"email"`
	Issuer      string          `json:"iss"`
	Audience    json.RawMessage `json:"aud"`
	Expires     int64           `json:"exp"`
	WorkspaceID string          `json:"chatgpt_account_id"`
	Auth        struct {
		WorkspaceID string `json:"chatgpt_account_id"`
	} `json:"https://api.openai.com/auth"`
	Organizations []struct {
		ID string `json:"id"`
	} `json:"organizations"`
}

func (i identityClaims) accountID() string {
	if i.WorkspaceID != "" {
		return i.WorkspaceID
	}
	if i.Auth.WorkspaceID != "" {
		return i.Auth.WorkspaceID
	}
	if len(i.Organizations) > 0 {
		return i.Organizations[0].ID
	}
	return ""
}

// Validate against the published OpenID issuer/JWKS. Merely decoding a JWT does
// not establish which account authorized a registration.
func (c *Client) validateIDToken(ctx context.Context, token, clientID string) (identityClaims, error) {
	var claims identityClaims
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return claims, fmt.Errorf("invalid ChatGPT ID token")
	}
	decode := base64.RawURLEncoding.DecodeString
	headerRaw, err := decode(parts[0])
	if err != nil {
		return claims, fmt.Errorf("invalid ChatGPT ID token header")
	}
	var header struct {
		Algorithm string `json:"alg"`
		KeyID     string `json:"kid"`
	}
	if json.Unmarshal(headerRaw, &header) != nil || header.Algorithm != "RS256" || header.KeyID == "" {
		return claims, fmt.Errorf("unsupported ChatGPT ID token signing key")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.jwksURL, nil)
	if err != nil {
		return claims, err
	}
	raw, err := c.request(req)
	if err != nil {
		return claims, err
	}
	var jwks struct {
		Keys []struct {
			KeyID     string `json:"kid"`
			Type      string `json:"kty"`
			Algorithm string `json:"alg"`
			Use       string `json:"use"`
			N         string `json:"n"`
			E         string `json:"e"`
		} `json:"keys"`
	}
	if json.Unmarshal(raw, &jwks) != nil {
		return claims, fmt.Errorf("invalid OpenAI signing keys")
	}
	var key *rsa.PublicKey
	for _, k := range jwks.Keys {
		if k.KeyID != header.KeyID || k.Type != "RSA" || (k.Algorithm != "" && k.Algorithm != "RS256") || (k.Use != "" && k.Use != "sig") {
			continue
		}
		n, nerr := decode(k.N)
		e, eerr := decode(k.E)
		if nerr != nil || eerr != nil || len(e) == 0 || len(e) > 4 {
			continue
		}
		exponent := int64(0)
		for _, b := range e {
			exponent = exponent<<8 | int64(b)
		}
		if exponent < 3 || exponent > 1<<31-1 {
			continue
		}
		key = &rsa.PublicKey{N: new(big.Int).SetBytes(n), E: int(exponent)}
		break
	}
	if key == nil {
		return claims, fmt.Errorf("ChatGPT ID token signing key not found")
	}
	signature, err := decode(parts[2])
	if err != nil {
		return claims, fmt.Errorf("invalid ChatGPT ID token signature")
	}
	sum := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	if rsa.VerifyPKCS1v15(key, crypto.SHA256, sum[:], signature) != nil {
		return claims, fmt.Errorf("ChatGPT ID token signature verification failed")
	}
	payload, err := decode(parts[1])
	if err != nil || json.Unmarshal(payload, &claims) != nil {
		return claims, fmt.Errorf("invalid ChatGPT ID token claims")
	}
	var audience []string
	if json.Unmarshal(claims.Audience, &audience) != nil {
		var one string
		_ = json.Unmarshal(claims.Audience, &one)
		audience = []string{one}
	}
	matched := false
	for _, aud := range audience {
		if aud == clientID {
			matched = true
		}
	}
	if !matched || claims.Issuer != c.issuer || claims.Subject == "" || claims.Expires <= time.Now().Unix() {
		return claims, fmt.Errorf("ChatGPT ID token identity validation failed")
	}
	return claims, nil
}
