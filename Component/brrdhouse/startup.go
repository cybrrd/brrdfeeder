// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"
)

// Numeric public projection only; absent observations are not zeroes. Untrusted
// strings cannot become HTML/SSE content through this diagnostics schema.
type startupGPS struct {
	SatellitesUsed *uint16  `json:"satellites_used"`
	SatellitesView *uint16  `json:"satellites_in_view"`
	Quality        *uint8   `json:"fix_quality"`
	Mode           *uint8   `json:"fix_mode"`
	MaxSNR         *float64 `json:"snr_max_dbhz"`
	AvgSNR         *float64 `json:"snr_avg_dbhz"`
	NMEAAge        *float64 `json:"nmea_age_secs"`
	HDOP           *float64 `json:"hdop"`
}

func (g *startupGPS) validate() {
	if g.HDOP != nil && !validHDOP(*g.HDOP) {
		g.HDOP = nil
	}
	if g.SatellitesUsed != nil && *g.SatellitesUsed > 99 {
		g.SatellitesUsed = nil
	}
	if g.SatellitesView != nil && *g.SatellitesView > 288 {
		g.SatellitesView = nil
	}
	if g.Quality != nil && *g.Quality > 8 {
		g.Quality = nil
	}
	if g.Mode != nil && (*g.Mode < 1 || *g.Mode > 3) {
		g.Mode = nil
	}
	if g.MaxSNR != nil && (*g.MaxSNR < 0 || *g.MaxSNR > 99) {
		g.MaxSNR = nil
	}
	if g.AvgSNR != nil && (*g.AvgSNR < 0 || *g.AvgSNR > 99) {
		g.AvgSNR = nil
	}
	if g.NMEAAge != nil && (*g.NMEAAge < 0 || *g.NMEAAge > 1e9) {
		g.NMEAAge = nil
	}
}

// A separate pre-engine writer, never an engine heartbeat. Even a fresh
// startup record is OFFLINE. Presence takes precedence over an old engine
// report; corrupt/stale records cannot turn into a green engine tick.
func readStartup(statusPath string, now time.Time) (view, bool) {
	v := offline(view{Checked: now.UTC().Format(time.RFC3339), Reason: "Engine startup status is unavailable or invalid."})
	f, err := os.OpenFile(filepath.Join(filepath.Dir(statusPath), "startup.json"), os.O_RDONLY|syscall.O_NONBLOCK|syscall.O_NOFOLLOW, 0)
	if os.IsNotExist(err) {
		return view{}, false
	}
	if err != nil {
		return v, true
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Size() > 4096 {
		return v, true
	}
	data, err := io.ReadAll(io.LimitReader(f, 4097))
	var s struct {
		Schema    int         `json:"schema_version"`
		State     string      `json:"state"`
		WrittenAt time.Time   `json:"written_at"`
		Interval  uint64      `json:"status_interval_secs"`
		GPS       *startupGPS `json:"gps"`
		Adapters  []string    `json:"usb_adapter_ids"`
	}
	if err != nil || len(data) > 4096 || json.Unmarshal(data, &s) != nil || s.Schema != 1 || s.Interval != 5 || s.WrittenAt.IsZero() {
		return v, true
	}
	if s.WrittenAt.After(now) || now.Sub(s.WrittenAt) > 3*time.Duration(s.Interval)*time.Second {
		v.Reason = "Engine startup status is stale. The GPS waiter is no longer reporting."
		return v, true
	}
	action := "The service will start the engine automatically after a measured GPS fix. You do not need to enter coordinates."
	switch s.State {
	case "gps-missing":
		v.Reason = "GPS not detected: plug in the GPS."
		if len(s.Adapters) <= 8 {
			var ids []string
			for _, id := range s.Adapters {
				if regexp.MustCompile(`^[0-9a-f]{4}:[0-9a-f]{4}$`).MatchString(id) {
					ids = append(ids, id)
				}
			}
			if len(ids) > 0 {
				v.Reason += " Serial adapter IDs seen: " + strings.Join(ids, ", ") + ". These are not automatically claimed as GPS."
			}
		}
	case "gps-waiting":
		v.Reason = "Waiting for GPS fix. Place the antenna with a clear view of the sky."
	case "gps-busy":
		v.Reason = "GPS is unavailable or in use by another process."
		action = "Ask your installer to check GPS device ownership. The service will retry without taking another process's device."
	case "gps-fix":
		v.Reason = "Measured GPS fix acquired; preparing engine startup. The engine is not yet confirmed running."
	default:
		return v, true
	}
	v.Repairs = []repair{{v.Reason, action}}
	if s.GPS == nil {
		s.GPS = &startupGPS{}
	}
	s.GPS.validate()
	v.StartupGPS = s.GPS
	return v, true
}
