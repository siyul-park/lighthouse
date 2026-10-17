package sdk_test

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

type transcript struct {
	Applies   []string   `json:"applies"`
	Exchanges []exchange `json:"exchanges"`
}

type exchange struct {
	Send   sdk.Message  `json:"send"`
	Expect *sdk.Message `json:"expect"`
}

// transcriptsGlob holds the recorded exchanges that every implementation of
// the protocol replays: the Rust server runs the same files.
const transcriptsGlob = "../../../protocol/conformance/*.json"

func matches(want, got any) bool {
	switch w := want.(type) {
	case string:
		_, isString := got.(string)
		return w == "$any" || (w == "$string" && isString) || w == got
	case map[string]any:
		g, ok := got.(map[string]any)
		if !ok || len(g) != len(w) {
			return false
		}
		for k, v := range w {
			if gv, ok := g[k]; !ok || !matches(v, gv) {
				return false
			}
		}
		return true
	case []any:
		g, ok := got.([]any)
		if !ok || len(g) != len(w) {
			return false
		}
		for i := range w {
			if !matches(w[i], g[i]) {
				return false
			}
		}
		return true
	default:
		return want == got
	}
}

// replayTranscripts replays every transcript for language plugins against the
// sdk server.
func replayTranscripts(t *testing.T) {
	files, err := filepath.Glob(transcriptsGlob)
	require.NoError(t, err)
	require.GreaterOrEqual(t, len(files), 5, "conformance transcripts went missing")
	for _, file := range files {
		body, err := os.ReadFile(file)
		require.NoError(t, err)
		var tr transcript
		require.NoError(t, json.Unmarshal(body, &tr))
		if !slices.Contains(tr.Applies, "language") {
			continue
		}
		t.Run(strings.TrimSuffix(filepath.Base(file), ".json"), func(t *testing.T) {
			var in strings.Builder
			for _, ex := range tr.Exchanges {
				in.WriteString(frame(t, ex.Send))
			}
			var out bytes.Buffer

			err := sdk.Serve(strings.NewReader(in.String()), &out, newHandler())

			if last := tr.Exchanges[len(tr.Exchanges)-1].Send; last.Method == sdk.MethodExit {
				require.NoError(t, err)
			}
			got := replies(t, out.String())
			for _, ex := range tr.Exchanges {
				if ex.Expect == nil {
					continue
				}
				require.NotEmpty(t, got, "a reply to %s is missing", ex.Send.Method)
				requireMatch(t, asValue(t, *ex.Expect), asValue(t, got[0]))
				got = got[1:]
			}
			require.Empty(t, got, "replies nobody expected")
		})
	}
}

// asValue is a message as the generic JSON value that requireMatch compares.
func asValue(t *testing.T, m sdk.Message) any {
	t.Helper()
	body, err := json.Marshal(m)
	require.NoError(t, err)
	var v any
	require.NoError(t, json.Unmarshal(body, &v))
	return v
}

// requireMatch compares an expected JSON value with an actual one: objects
// and arrays must match member for member, "$any" matches any value and
// "$string" any string.
func requireMatch(t *testing.T, want, got any) {
	t.Helper()
	require.True(t, matches(want, got), "want %v, got %v", want, got)
}
