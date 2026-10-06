// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"errors"
	"flag"
	"math"
)

const gpsPlacement = "GPS satellites are mostly in the southern sky. Put the GPS where it can see south and overhead. North-facing windows, metal roofs and tree cover can prevent a fix."

// One policy for both phases. These are reception guidance, not accuracy guarantees.
type gpsPolicy struct {
	GoodSats, MarginalSats int
	GoodHDOP, MarginalHDOP float64
}

func defaultGPSPolicy() gpsPolicy { return gpsPolicy{6, 4, 2, 5} }

func (p *gpsPolicy) flags(f *flag.FlagSet) {
	f.IntVar(&p.GoodSats, "gps-good-min-sats", p.GoodSats, "minimum used satellites for Good GPS")
	f.IntVar(&p.MarginalSats, "gps-marginal-min-sats", p.MarginalSats, "minimum used satellites for Marginal GPS")
	f.Float64Var(&p.GoodHDOP, "gps-good-max-hdop", p.GoodHDOP, "maximum HDOP for Good GPS")
	f.Float64Var(&p.MarginalHDOP, "gps-marginal-max-hdop", p.MarginalHDOP, "maximum HDOP for Marginal GPS")
}

func (p gpsPolicy) validate() error {
	if p.MarginalSats < 1 || p.GoodSats < p.MarginalSats || p.GoodSats > 99 ||
		!validHDOP(p.GoodHDOP) || !validHDOP(p.MarginalHDOP) || p.GoodHDOP > p.MarginalHDOP {
		return errors.New("invalid GPS rating thresholds: require 1 <= marginal sats <= good sats <= 99 and 0 < good HDOP <= marginal HDOP <= 999")
	}
	return nil
}

func validHDOP(n float64) bool { return n > 0 && n <= 999 && !math.IsNaN(n) && !math.IsInf(n, 0) }

func (p gpsPolicy) rate(fix bool, sats int, hdop *float64) string {
	if !fix {
		return "No fix"
	}
	if sats < 0 || sats > 99 || hdop == nil || !validHDOP(*hdop) {
		return "Poor"
	}
	if sats >= p.GoodSats && *hdop <= p.GoodHDOP {
		return "Good"
	}
	if sats >= p.MarginalSats && *hdop <= p.MarginalHDOP {
		return "Marginal"
	}
	return "Poor"
}

func (v *view) rateGPS(p gpsPolicy) {
	v.GPSPlacement = gpsPlacement
	if g := v.StartupGPS; g != nil {
		fix := g.Quality != nil && *g.Quality > 0 && (g.Mode == nil || *g.Mode > 1)
		sats := -1
		if g.SatellitesUsed != nil {
			sats = int(*g.SatellitesUsed)
		}
		v.GPSRating = p.rate(fix, sats, g.HDOP)
		v.GPSActivity = "The service is waiting for a measured GPS fix before starting the engine."
		if fix {
			v.GPSActivity = "A fix was reported; the service is preparing engine startup. The engine is not yet confirmed running."
		}
		return
	}
	if !v.Live {
		return
	} // never grade stale/missing data
	fix, sats := false, -1
	var hdop *float64
	if g := v.Status.Inventory.GPS; g != nil {
		fix = g.State == "healthy" && g.Quality != nil && *g.Quality > 0 && *g.Quality <= 8
		if g.Satellites != nil {
			sats = int(*g.Satellites)
		}
		hdop = g.HDOP
	}
	v.GPSRating = p.rate(fix, sats, hdop)
	v.GPSActivity = "The engine is running on saved position; position preserved, GPS not live."
	if fix {
		v.GPSActivity = "The engine is running with a reported GPS fix. Lower HDOP generally means better satellite geometry; this rating is not a position-accuracy guarantee."
	}
	if fix && (sats < 0 || hdop == nil) {
		v.GPSActivity += " Reception is conservatively rated Poor because measurements are missing."
	}
}
