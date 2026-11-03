// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import "errors"

// A missing fresh, explicit trust report defers expiry-sensitive decisions.
// It is nonzero for CLI callers; only the two poll units accept it as success.
var errClockUntrusted = errors.New("release clock untrusted: engine has not reported a fresh trusted clock (os_clock_trusted=true); update deferred. If the engine is not running, re-run the installer to update")

func exitCode(err error) int {
	if err == nil {
		return 0
	}
	if onlyClockDefer(err) {
		return 75 // EX_TEMPFAIL; matched by the two poll units' SuccessExitStatus.
	}
	return 1
}

func onlyClockDefer(err error) bool {
	if err == errClockUntrusted {
		return true
	}
	// execute joins the result with failures writing attempt receipts. An
	// errors.Is check alone would hide those real failures behind a defer.
	switch wrapped := err.(type) {
	case interface{ Unwrap() []error }:
		children := wrapped.Unwrap()
		if len(children) == 0 {
			return false
		}
		for _, child := range children {
			if !onlyClockDefer(child) {
				return false
			}
		}
		return true
	case interface{ Unwrap() error }:
		return onlyClockDefer(wrapped.Unwrap())
	default:
		return false
	}
}
