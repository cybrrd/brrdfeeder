// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

func releaseSignerClient(ca string) (*http.Client, error) {
	roots, e := x509.SystemCertPool()
	if e != nil {
		return nil, e
	}
	if ca != "" {
		b, e := readBounded(ca, 1<<20)
		if e != nil {
			return nil, e
		}
		if !roots.AppendCertsFromPEM(b) {
			return nil, errors.New("invalid signer CA")
		}
	}
	return &http.Client{Timeout: 75 * time.Second, Transport: &http.Transport{TLSClientConfig: &tls.Config{MinVersion: tls.VersionTLS12, RootCAs: roots}}, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("signer redirect refused") }}, nil
}
func signReleaseRemote(m Release, endpoint, tokenFile string, client *http.Client, keys []ed25519.PublicKey) (Release, error) {
	u, e := url.Parse(endpoint)
	if e != nil || u.Scheme != "https" || u.Host == "" || u.User != nil || u.Path != "/sign/release" || u.RawQuery != "" || u.Fragment != "" {
		return Release{}, errors.New("HTTPS /sign/release endpoint required")
	}
	raw, e := json.Marshal(m)
	if e != nil {
		return Release{}, e
	}
	req, e := http.NewRequest(http.MethodPost, u.String(), bytes.NewReader(raw))
	if e != nil {
		return Release{}, e
	}
	req.Header.Set("Content-Type", "application/json")
	if tokenFile != "" {
		b, e := readBounded(tokenFile, 4096)
		if e != nil {
			return Release{}, e
		}
		token := strings.TrimSpace(string(b))
		if token == "" || strings.ContainsAny(token, "\r\n") {
			return Release{}, errors.New("invalid signer token file")
		}
		req.Header.Set("Authorization", "Bearer "+token)
	}
	res, e := client.Do(req)
	if e != nil {
		return Release{}, e
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return Release{}, errors.New("signer refused release (no output written)")
	}
	b, e := io.ReadAll(io.LimitReader(res.Body, maxDocument+1))
	if e != nil {
		return Release{}, e
	}
	signed, e := verifyRelease(b, keys, time.Now())
	if e != nil {
		return Release{}, e
	}
	if !bytes.Equal(signed.canonical(), m.canonical()) {
		return Release{}, errors.New("signer changed release fields")
	}
	return signed, nil
}
