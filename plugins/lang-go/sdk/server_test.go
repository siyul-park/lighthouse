package sdk_test

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

type stub struct {
	index func(sdk.IndexParams) (sdk.IndexResult, error)
}

func (stub) Name() string                     { return "x" }
func (stub) Version() string                  { return "1" }
func (stub) Manifest() []sdk.LanguageManifest { return []sdk.LanguageManifest{{ID: "stub"}} }

func (s stub) Index(p sdk.IndexParams) (sdk.IndexResult, error) { return s.index(p) }

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

func newHandler() stub {
	return stub{
		index: func(p sdk.IndexParams) (sdk.IndexResult, error) {
			if p.Language == "panic" {
				panic("boom")
			}
			if p.Language == "fail" {
				return sdk.IndexResult{}, errors.New("no")
			}
			return sdk.IndexResult{Fragments: []sdk.Fragment{}, Notices: []string{p.Project.Root}, Incomplete: []sdk.Incomplete{}}, nil
		},
	}
}

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

func TestServe(t *testing.T) {
	t.Run("answers requests and stops after shutdown and exit", func(t *testing.T) {
		var in strings.Builder
		in.WriteString(request(t, 1, sdk.MethodInitialize, sdk.InitializeParams{ProtocolVersion: sdk.ProtocolVersion, WorkspaceFolders: []sdk.WorkspaceFolder{{URI: "file:///r%20x/", Name: "r x"}}}))
		in.WriteString(request(t, 2, sdk.MethodIndex, sdk.IndexParams{Project: sdk.ProjectRef{Root: "/r"}, Language: "go"}))
		in.WriteString(request(t, 3, "nope", struct{}{}))
		in.WriteString(request(t, 4, sdk.MethodIndex, sdk.IndexParams{Language: "panic"}))
		in.WriteString(request(t, 5, sdk.MethodIndex, sdk.IndexParams{Language: "fail"}))
		in.WriteString(request(t, 6, sdk.MethodIndex, "not an object"))
		in.WriteString(request(t, 7, sdk.MethodShutdown, nil))
		in.WriteString(frame(t, sdk.Message{Method: sdk.MethodExit}))
		var out bytes.Buffer

		err := sdk.Serve(strings.NewReader(in.String()), &out, newHandler())

		require.NoError(t, err)
		got := replies(t, out.String())
		require.Len(t, got, 7)
		var init sdk.InitializeResult
		require.NoError(t, json.Unmarshal(got[0].Result, &init))
		require.Equal(t, "x", init.ServerInfo.Name)
		require.Equal(t, "stub", init.Capabilities.Languages[0].ID)
		require.JSONEq(t, `{"fragments":[],"notices":["/r"],"incomplete":[]}`, string(got[1].Result))
		for i, code := range map[int]int{2: -32601, 3: -32603, 4: -32803, 5: -32602} {
			require.NotNil(t, got[i].Error, "reply %d", i)
			require.Equal(t, code, got[i].Error.Code, "reply %d", i)
		}
		require.Contains(t, got[3].Error.Message, "panic: boom")
		require.Equal(t, "null", string(got[6].Result))
	})

	t.Run("refuses a request before initialize until one succeeds", func(t *testing.T) {
		var in strings.Builder
		in.WriteString(request(t, 1, sdk.MethodIndex, sdk.IndexParams{Language: "go"}))
		in.WriteString(request(t, 2, "nope", struct{}{}))
		in.WriteString(request(t, 3, sdk.MethodInitialize, sdk.InitializeParams{ProtocolVersion: "0.0"}))
		in.WriteString(request(t, 4, sdk.MethodIndex, sdk.IndexParams{Language: "go"}))
		in.WriteString(request(t, 5, sdk.MethodInitialize, struct{}{}))
		in.WriteString(request(t, 6, sdk.MethodShutdown, nil))
		in.WriteString(frame(t, sdk.Message{Method: sdk.MethodExit}))
		var out bytes.Buffer

		require.Error(t, sdk.Serve(strings.NewReader(in.String()), &out, newHandler()))

		got := replies(t, out.String())
		codes := make([]int, len(got))
		for i, reply := range got {
			require.NotNil(t, reply.Error, "reply %d", i)
			codes[i] = reply.Error.Code
		}
		require.Equal(t, []int{-32002, -32002, -32803, -32002, -32602, -32002}, codes)
	})

	t.Run("replays the recorded transcripts", replayTranscripts)

	t.Run("reports an input that ends or exits too early", func(t *testing.T) {
		var out bytes.Buffer

		endErr := sdk.Serve(strings.NewReader(""), &out, newHandler())
		exitErr := sdk.Serve(strings.NewReader(frame(t, sdk.Message{Method: sdk.MethodExit})), &out, newHandler())

		require.Error(t, endErr, "EOF before exit must be an error")
		require.Error(t, exitErr, "exit without shutdown must be an error")
	})

	t.Run("answers garbage with a parse error and stops", func(t *testing.T) {
		var out bytes.Buffer

		err := sdk.Serve(strings.NewReader("Content-Length: 2\r\n\r\n{]"), &out, newHandler())

		require.Error(t, err, "garbage must end the run")
		got := replies(t, out.String())
		require.Len(t, got, 1)
		require.NotNil(t, got[0].Error)
		require.Equal(t, -32700, got[0].Error.Code)
	})
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

func request(t *testing.T, id int, method string, params any) string {
	t.Helper()
	raw, err := json.Marshal(params)
	require.NoError(t, err)
	rawID := json.RawMessage(strconv.Itoa(id))
	return frame(t, sdk.Message{ID: &rawID, Method: method, Params: raw})
}

func frame(t *testing.T, m sdk.Message) string {
	t.Helper()
	var b bytes.Buffer
	require.NoError(t, sdk.WriteMessage(&b, &m))
	return b.String()
}

func replies(t *testing.T, out string) []sdk.Message {
	t.Helper()
	r := bufio.NewReader(strings.NewReader(out))
	var all []sdk.Message
	for {
		m, err := sdk.ReadMessage(r)
		if errors.Is(err, io.EOF) {
			return all
		}
		require.NoError(t, err)
		all = append(all, *m)
	}
}
