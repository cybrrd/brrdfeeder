// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"io"
	"os"
	"strings"
	"syscall"
	"time"
)

// Public projection of D17 schema 1. Never pass a generic JSON map or the raw
// heartbeat to a template. Identity, position, account and diagnostic fields
// intentionally have no destination here. Strings remain untrusted text.
type status struct {
	Schema    int       `json:"schema_version"`
	WrittenAt time.Time `json:"written_at"`
	Interval  uint64    `json:"status_interval_secs"`
	Heartbeat struct {
		Memory         *memoryStatus `json:"memory"`
		ProductVersion string        `json:"product_version"`
		Revision       string        `json:"engine_version"`
		Build          uint64        `json:"build_seq"`
		Digest         string        `json:"image_digest"`
		Radio          string        `json:"radio_status"`
		ClockTrusted   *bool         `json:"os_clock_trusted"`
	} `json:"heartbeat"`
	Inventory struct {
		Capture []struct {
			Interface string `json:"interface"`
			Driver    string `json:"driver"`
			Monitor   *bool  `json:"monitor_mode"`
			Channel   uint32 `json:"current_channel"`
		} `json:"capture"`
		BLE *struct {
			Address       string            `json:"bd_addr"`
			USB           string            `json:"usb_id"`
			Observed      string            `json:"observed_at"`
			State         string            `json:"state"`
			CurrentRFKill *blockObservation `json:"current_rfkill"`
			Rfkill        *struct {
				Soft     bool   `json:"soft_blocked"`
				Hard     bool   `json:"hard_blocked"`
				Observed string `json:"observed_at"`
			} `json:"rfkill"`
		} `json:"rid_ble"`
		GPS *struct {
			Device     string   `json:"device"`
			State      string   `json:"state"`
			Quality    *uint8   `json:"fix_quality"`
			Satellites *uint8   `json:"sat_count"`
			HDOP       *float64 `json:"hdop"`
		} `json:"gps"`
	} `json:"inventory"`
	Links struct {
		NATS      string `json:"nats_state"`
		Published string `json:"last_successful_publish"`
		LastFrame string `json:"last_frame_observed"`
		Frames    uint64 `json:"frames_last_hour"`
	} `json:"links"`
}

type blockObservation struct {
	Soft     *bool  `json:"soft_blocked"`
	Hard     *bool  `json:"hard_blocked"`
	Observed string `json:"observed_at"`
}

type memoryStatus struct {
	EngineRSS  *uint64 `json:"engine_rss_bytes"`
	ConsoleRSS *uint64 `json:"console_rss_bytes"`
	Events     *uint64 `json:"memory_cap_events"`
}

type repair struct{ Problem, Action string }
type view struct {
	Memory                               *memoryStatus
	Live                                 bool
	Reason                               string
	Checked                              string
	Age                                  string
	Budget                               uint64
	Status                               status
	StartupGPS                           *startupGPS
	Repairs                              []repair
	Radio, GPS, NATS, RFKill             string
	BLE, RFKillObserved                  string
	GPSRating, GPSPlacement, GPSActivity string
}

const maxStatusBytes = 1 << 20
const maxInterval = uint64((1<<63 - 1) / int64(time.Second) / 3)

// sanitize drops bytes that can break SSE event framing or parsing: bare CR
// (the WHATWG SSE spec terminates lines on CR as well as LF) and NUL. Newline
// and tab are kept — they are inert in HTML text context, the SSE writer
// already neutralizes newline, and displayed payloads legitimately contain them.
func sanitize(s string) string {
	return strings.Map(func(r rune) rune {
		if r == '\r' || r == 0 {
			return -1
		}
		return r
	}, s)
}

func readStatus(path string, now time.Time) view {
	return readStatusWithMemory(path, now, readHostMemory("/run/brrdfeeder-memory/host.json", "/proc/sys/kernel/random/boot_id", "/proc/uptime"))
}

func readStatusWithMemory(path string, now time.Time, hostMemory *memoryStatus) view {
	if v, present := readStartup(path, now); present {
		v.Memory = hostMemory
		memoryRepair(&v)
		return v
	}
	v := view{Memory: hostMemory, Checked: now.UTC().Format(time.RFC3339), Reason: "The engine status file is missing or unreadable."}
	// Reopen the path: the engine atomically renames new files into this directory.
	f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NONBLOCK|syscall.O_NOFOLLOW, 0)
	if err != nil {
		return offline(v)
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Size() > maxStatusBytes {
		v.Reason = "The engine status file is invalid."
		return offline(v)
	}
	b, err := io.ReadAll(io.LimitReader(f, maxStatusBytes+1))
	var s status
	if err != nil || len(b) > maxStatusBytes || json.Unmarshal(b, &s) != nil || s.Schema != 1 || s.WrittenAt.IsZero() || s.Interval == 0 || s.Interval > maxInterval {
		v.Reason = "The engine status file is invalid or uses an unsupported format."
		return offline(v)
	}
	if s.WrittenAt.After(now) || (s.Heartbeat.ClockTrusted != nil && !*s.Heartbeat.ClockTrusted) {
		v.Reason = "The engine clock cannot establish a trustworthy status time."
		if s.Inventory.GPS != nil && s.Inventory.GPS.State == "failed" {
			v.Reason += " The last engine report says position preserved, GPS not live. Current report freshness is unverified."
		}
		return offline(v)
	}
	age := now.Sub(s.WrittenAt)
	budget := time.Duration(s.Interval) * 3 * time.Second
	v.Age = age.Round(time.Second).String()
	v.Budget = s.Interval * 3
	if age > budget { // FRESHNESS CONTRACT: mutate only this comparison in the proof.
		v.Reason = "The engine has stopped reporting. Its last status is stale."
		return offline(v)
	}
	for i := range s.Inventory.Capture {
		s.Inventory.Capture[i].Interface = sanitize(s.Inventory.Capture[i].Interface)
		s.Inventory.Capture[i].Driver = sanitize(s.Inventory.Capture[i].Driver)
	}
	if b := s.Inventory.BLE; b != nil {
		b.Address, b.USB, b.Observed = sanitize(b.Address), sanitize(b.USB), sanitize(b.Observed)
		b.State = sanitize(b.State)
		if b.CurrentRFKill != nil {
			b.CurrentRFKill.Observed = sanitize(b.CurrentRFKill.Observed)
		}
		if b.Rfkill != nil {
			b.Rfkill.Observed = sanitize(b.Rfkill.Observed)
		}
	}
	if g := s.Inventory.GPS; g != nil {
		g.Device, g.State = sanitize(g.Device), sanitize(g.State)
		if g.HDOP != nil && !validHDOP(*g.HDOP) {
			g.HDOP = nil
		}
		if g.Quality != nil && *g.Quality > 8 {
			g.Quality = nil
		}
		if g.Satellites != nil && *g.Satellites > 99 {
			g.Satellites = nil
		}
	}
	s.Heartbeat.ProductVersion, s.Heartbeat.Revision, s.Heartbeat.Digest, s.Heartbeat.Radio =
		sanitize(s.Heartbeat.ProductVersion), sanitize(s.Heartbeat.Revision), sanitize(s.Heartbeat.Digest), sanitize(s.Heartbeat.Radio)
	s.Links.NATS, s.Links.Published, s.Links.LastFrame =
		sanitize(s.Links.NATS), sanitize(s.Links.Published), sanitize(s.Links.LastFrame)
	v.Live, v.Status = true, s
	if v.Memory == nil {
		v.Memory = s.Heartbeat.Memory
	}
	if v.Memory != nil && s.Heartbeat.Memory != nil {
		memory := *v.Memory
		memory.EngineRSS = s.Heartbeat.Memory.EngineRSS
		v.Memory = &memory
	}
	v.Reason = "The engine is reporting current status."
	add := func(problem, action string) { v.Repairs = append(v.Repairs, repair{problem, action}) }
	memoryRepair(&v)
	v.Radio = "Not confirmed"
	switch s.Heartbeat.Radio {
	case "up":
		v.Radio = "Capturing"
	case "recovering":
		v.Radio = "Recovering"
		add("Wi-Fi capture is recovering.", "Wait briefly. If this continues, check that the USB capture adapter is firmly connected.")
	default:
		add("Wi-Fi capture is not confirmed.", "Check the USB capture adapter and cable. The Pi's built-in Wi-Fi does not provide monitor capture.")
	}
	if len(s.Inventory.Capture) == 0 {
		add("No capture interface was reported.", "Check the USB capture adapter and ask your installer to verify the engine's interface setting.")
	}
	v.RFKill = "Unknown (no current observation)"
	v.BLE = "Not reported"
	if b := s.Inventory.BLE; b != nil {
		switch b.State {
		case "healthy":
			v.BLE = "Healthy"
		case "degraded":
			v.BLE = "Degraded"
		case "initializing":
			v.BLE = "Initializing"
		case "failed":
			v.BLE = "Failed"
		}
		blocked := false
		if r := b.CurrentRFKill; r != nil && r.Soft != nil && r.Hard != nil {
			observed, err := time.Parse(time.RFC3339Nano, r.Observed)
			// The status-file clock does not refresh an older rfkill observation.
			if err == nil && !observed.After(s.WrittenAt) && now.Sub(observed) <= budget {
				v.RFKill = "Not blocked"
				v.RFKillObserved = r.Observed
				blocked = *r.Soft || *r.Hard
				if blocked {
					v.RFKill = "Blocked"
					add("Bluetooth is currently blocked.", "Check any physical radio switch. Ask your installer to check the Bluetooth block setting; this page cannot change it.")
				}
			}
		}
		if b.State == "failed" && !blocked {
			add("Bluetooth reception has failed.", "Check the Bluetooth adapter and ask your installer to inspect the engine's current Bluetooth state.")
		}
	} else {
		add("No Bluetooth inventory was reported.", "If Bluetooth reception is expected, check the adapter and ask your installer to verify Bluetooth capture is enabled.")
	}
	v.GPS = "No confirmed fix"
	if g := s.Inventory.GPS; g != nil && g.State == "healthy" && g.Quality != nil && *g.Quality > 0 {
		v.GPS = "Fix available"
	} else {
		add("GPS has no confirmed fix.", gpsPlacement)
	}
	v.NATS = "Disconnected or unknown"
	if strings.EqualFold(s.Links.NATS, "connected") {
		v.NATS = "Connected"
	} else {
		add("The engine is not connected to the messaging service.", "Check the unit's network cable or Wi-Fi and internet access. If other devices are online, ask your installer to check the engine connection.")
	}
	if s.Links.Frames == 0 {
		add("No radio activity was counted in the last hour.", "This can be normal in a quiet area. If traffic is expected, check the capture adapter and antenna. Activity counts are raw packets, not a drone count.")
	}
	return v
}

func offline(v view) view {
	v.Repairs = []repair{{"Engine offline or status unavailable.", "Check the unit's power and network. If this persists, ask your installer to check the engine and its status-file setup. This page cannot restart it."}}
	memoryRepair(&v)
	return v
}
