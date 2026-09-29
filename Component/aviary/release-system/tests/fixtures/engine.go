// Synthetic status writer for worldport-only container lifecycle proofs.
// No radio, broker, credential, or production engine behavior is represented.
package main

import (
	"encoding/json"
	"os"
	"strconv"
	"time"
)

func main() {
	for {
		seq, _ := strconv.Atoi(os.Getenv("D44_SEQ"))
		radio := "up"
		if os.Getenv("D44_BAD") == "true" {
			radio = "error"
		}
		b, _ := json.Marshal(map[string]any{"schema_version": 1, "written_at": time.Now().UTC().Format(time.RFC3339Nano), "status_interval_secs": 1,
			"heartbeat": map[string]any{"node_id": "brrdg3s1", "image_digest": os.Getenv("D44_DIGEST"), "engine_version": os.Getenv("D44_VERSION"), "build_seq": seq, "radio_status": radio, "os_clock_trusted": true}})
		_ = os.WriteFile("/state/status.next", b, 0644)
		_ = os.Rename("/state/status.next", "/state/status.json")
		time.Sleep(500 * time.Millisecond)
	}
}
