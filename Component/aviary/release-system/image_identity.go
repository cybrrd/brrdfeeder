// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
package main

import (
	"encoding/json"
	"errors"
	"strconv"
	"strings"
)

// Metadata is deliberately not added to Image/Checkpoint's persisted wire
// schema. Older helpers can still read a journal when recovering retained pins.
func (u *Updater) verifyImageTuple(console bool, im Image, digest, revision string, build uint64) error {
	repo, buildLabel := repository, "com.macawi.brrdfeeder.build_seq"
	if console {
		repo, buildLabel = consoleRepository, "com.macawi.brrdhouse.build_seq"
	}
	if im.Digest != digest || !contains(im.RepoDigests, repo+"@"+digest) {
		return errors.New("candidate digest identity mismatch")
	}
	raw, e := u.componentRun(console, "podman", "image", "inspect", im.ID)
	if e != nil {
		return e
	}
	var images []struct {
		Image
		Labels map[string]string `json:"Labels"`
	}
	if json.Unmarshal(raw, &images) != nil || len(images) != 1 {
		return errors.New("invalid candidate image metadata")
	}
	got := images[0]
	if strings.TrimPrefix(got.ID, "sha256:") != im.ID || got.Digest != digest || !contains(got.RepoDigests, repo+"@"+digest) || got.Labels["org.opencontainers.image.revision"] != revision || got.Labels[buildLabel] != strconv.FormatUint(build, 10) {
		return errors.New("candidate image revision/build identity mismatch")
	}
	return nil
}
