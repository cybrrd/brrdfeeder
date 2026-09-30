// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"bytes"
	"context"
	"embed"
	"errors"
	"flag"
	"fmt"
	"html/template"
	"log"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"
)

//go:embed web/* LICENSE licenses/*
var assets embed.FS
var templates = template.Must(template.New("").Funcs(template.FuncMap{"deref": func(v *bool) bool { return v != nil && *v }}).ParseFS(assets, "web/*.html"))

type console struct {
	path    string
	hosts   map[string]bool
	now     func() time.Time
	period  time.Duration
	streams chan struct{}
	gps     gpsPolicy
}

// Exact HTTP authorities (including port). No suffix matching, request-derived
// allowlists, forwarded-header trust, or blanket acceptance of private IPs.
func authority(raw string) (string, error) {
	if raw == "" || strings.ContainsAny(raw, "/\\@?#%\t\r\n ") {
		return "", errors.New("invalid host")
	}
	host := raw
	port := ""
	if strings.HasPrefix(raw, "[") || strings.Contains(raw, ":") {
		var err error
		host, port, err = net.SplitHostPort(raw)
		if err != nil {
			return "", err
		}
		n, err := strconv.Atoi(port)
		if err != nil || n < 1 || n > 65535 || strconv.Itoa(n) != port {
			return "", errors.New("invalid port")
		}
	}
	host = strings.ToLower(host)
	if ip, err := netip.ParseAddr(host); err == nil {
		host = ip.String()
	} else {
		if len(host) > 253 {
			return "", errors.New("invalid hostname")
		}
		for _, label := range strings.Split(host, ".") {
			if label == "" || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
				return "", errors.New("invalid hostname")
			}
			for _, c := range label {
				if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-') {
					return "", errors.New("invalid hostname")
				}
			}
		}
	}
	if port != "" {
		return net.JoinHostPort(host, port), nil
	}
	return host, nil
}

func newConsole(path, allowed string) (*console, error) {
	c := &console{path: path, hosts: map[string]bool{}, now: time.Now, period: time.Second, streams: make(chan struct{}, 32), gps: defaultGPSPolicy()}
	for _, raw := range strings.Split(allowed, ",") {
		host, err := authority(strings.TrimSpace(raw))
		if err != nil {
			return nil, fmt.Errorf("allowed-hosts: %w", err)
		}
		c.hosts[host] = true
	}
	return c, nil
}

func (c *console) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
	host, err := authority(r.Host)
	if err != nil || !c.hosts[host] {
		http.Error(w, "Host not allowed", http.StatusMisdirectedRequest)
		return
	}
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		w.Header().Set("Allow", "GET, HEAD")
		http.Error(w, "Read-only console", http.StatusMethodNotAllowed)
		return
	}
	if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
		http.Error(w, "Cross-site request denied", http.StatusForbidden)
		return
	}
	if origin := r.Header.Get("Origin"); origin != "" && origin != "http://"+r.Host {
		http.Error(w, "Origin denied", http.StatusForbidden)
		return
	}
	switch r.URL.Path {
	case "/", "/status":
		name := "page"
		if r.URL.Path == "/status" {
			name = "status"
		}
		var b bytes.Buffer
		if err := templates.ExecuteTemplate(&b, name, c.snapshot()); err != nil {
			http.Error(w, "Unable to render status", 500)
			return
		}
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		if r.Method != http.MethodHead {
			_, _ = w.Write(b.Bytes())
		}
	case "/events":
		c.events(w, r)
	case "/LICENSE":
		b, _ := assets.ReadFile("LICENSE")
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		if r.Method != http.MethodHead {
			_, _ = w.Write(b)
		}
	case "/web/style.css", "/web/htmx.min.js", "/web/sse.js", "/web/live.js":
		http.FileServer(http.FS(assets)).ServeHTTP(w, r)
	default:
		http.NotFound(w, r)
	}
}

func (c *console) snapshot() view {
	v := readStatus(c.path, c.now())
	v.rateGPS(c.gps)
	return v
}

func (c *console) events(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodHead {
		w.Header().Set("Content-Type", "text/event-stream")
		return
	}
	select {
	case c.streams <- struct{}{}:
		defer func() { <-c.streams }()
	default:
		http.Error(w, "Too many viewers; retry shortly", 503)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("X-Accel-Buffering", "no")
	ticker := time.NewTicker(c.period)
	defer ticker.Stop()
	rc := http.NewResponseController(w)
	for {
		var b bytes.Buffer
		if err := templates.ExecuteTemplate(&b, "status", c.snapshot()); err != nil {
			return
		}
		// Encode every line so hostile newlines cannot create additional SSE
		// events, and neutralise CR as well: the WHATWG SSE spec terminates
		// lines on CR too, so a bare CR could split events and inject protocol
		// lines. The projection strips CR from status values; this writer is
		// the second layer that stays safe regardless of input.
		_ = rc.SetWriteDeadline(time.Now().Add(5 * time.Second))
		if _, err := fmt.Fprintf(w, "event: status\ndata: %s\n\n",
			strings.ReplaceAll(strings.ReplaceAll(b.String(), "\r", ""), "\n", "\ndata: ")); err != nil {
			return
		}
		if rc.Flush() != nil {
			return
		}
		select {
		case <-r.Context().Done():
			return
		case <-ticker.C:
		}
	}
}

func main() {
	listen := flag.String("listen", "127.0.0.1:8080", "literal LAN IP:port; never a wildcard address")
	hosts := flag.String("allowed-hosts", "127.0.0.1:8080,localhost:8080", "comma-separated exact HTTP authorities, including port")
	path := flag.String("status-file", "/status/status.json", "read-only engine status file")
	policy := defaultGPSPolicy()
	policy.flags(flag.CommandLine)
	flag.Parse()
	if err := policy.validate(); err != nil {
		log.Fatal(err)
	}
	addr, err := netip.ParseAddrPort(*listen)
	if err != nil || addr.Port() == 0 || addr.Addr().IsUnspecified() || addr.Addr().IsMulticast() {
		log.Fatal("listen must be a specific unicast IP and nonzero port")
	}
	c, err := newConsole(*path, *hosts)
	if err != nil {
		log.Fatal(err)
	}
	c.gps = policy
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	srv := &http.Server{Addr: *listen, Handler: c, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 10 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 8192, BaseContext: func(net.Listener) context.Context { return ctx }}
	go func() {
		<-ctx.Done()
		shutdown, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdown)
	}()
	log.Printf("BRRDhouse read-only HTTP console listening on %s", *listen)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatal(err)
	}
}
