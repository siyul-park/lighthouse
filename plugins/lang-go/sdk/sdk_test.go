package sdk_test

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
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

func TestWriteMessage(t *testing.T) {
	id := json.RawMessage("7")
	var out bytes.Buffer

	err := sdk.WriteMessage(&out, &sdk.Message{ID: &id, Result: json.RawMessage(`"日本語"`)})

	require.NoError(t, err)
	header, body, _ := strings.Cut(out.String(), "\r\n\r\n")
	require.Equal(t, "Content-Length: "+strconv.Itoa(len(body)), header)
	require.Equal(t, `{"jsonrpc":"2.0","id":7,"result":"日本語"}`, body)

	t.Run("result and error", func(t *testing.T) {
		id := json.RawMessage("7")
		var out bytes.Buffer
		result := json.RawMessage(`{"a":[1,2]}`)
		require.True(t, json.Valid(result))

		err := sdk.WriteMessage(&out, &sdk.Message{ID: &id, Result: result, Error: &sdk.ResponseError{Code: 1, Message: "x"}})

		require.NoError(t, err)
		header, body, _ := strings.Cut(out.String(), "\r\n\r\n")
		require.Equal(t, "Content-Length: "+strconv.Itoa(len(body)), header)
		require.True(t, json.Valid([]byte(body)), body)
		var got sdk.Message
		require.NoError(t, json.Unmarshal([]byte(body), &got))
		require.JSONEq(t, string(result), string(got.Result))
		require.Equal(t, &sdk.ResponseError{Code: 1, Message: "x"}, got.Error)
	})

	t.Run("content length", func(t *testing.T) {
		for _, tc := range []struct {
			name   string
			result json.RawMessage
		}{
			{"null", json.RawMessage("null")},
			{"multibyte", json.RawMessage(`"日本語"`)},
			{"large", json.RawMessage(`"` + strings.Repeat("x", 100000) + `"`)},
			{"none", nil},
		} {
			t.Run(tc.name, func(t *testing.T) {
				id := json.RawMessage("12")
				var out bytes.Buffer
				require.True(t, tc.result == nil || json.Valid(tc.result))

				require.NoError(t, sdk.WriteMessage(&out, &sdk.Message{ID: &id, Result: tc.result}))

				raw := out.Bytes()
				sep := bytes.Index(raw, []byte("\r\n\r\n"))
				require.Equal(t, "Content-Length: "+strconv.Itoa(len(raw)-sep-4), string(raw[:sep]))
				require.True(t, json.Valid(raw[sep+4:]))
				m, err := sdk.ReadMessage(bufio.NewReader(&out))
				require.NoError(t, err)
				require.Equal(t, string(tc.result), string(m.Result))
			})
		}
	})
}

func TestReadMessage(t *testing.T) {
	for _, tc := range []struct {
		name    string
		input   string
		wantEOF bool
		wantErr bool
		want    string
	}{
		{name: "reads a framed message", input: "Content-Length: 45\r\n\r\n" + `{"jsonrpc":"2.0","id":7,"result":"日本語"}`, want: `"日本語"`},
		{name: "ends cleanly at empty input", input: "", wantEOF: true},
		{name: "rejects a missing length", input: "Content-Type: x\r\n\r\n{}", wantErr: true},
		{name: "rejects a malformed length", input: "Content-Length: nope\r\n\r\n", wantErr: true},
		{name: "rejects a short body", input: "Content-Length: 9\r\n\r\n{}", wantErr: true},
		{name: "rejects a malformed body", input: "Content-Length: 2\r\n\r\n{]", wantErr: true},
		{name: "rejects a header block without end", input: "Content-Length: 2\r\n", wantErr: true},
		{name: "rejects an oversized body", input: "Content-Length: 999999999999\r\n\r\n", wantErr: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, err := sdk.ReadMessage(bufio.NewReader(strings.NewReader(tc.input)))

			require.Equal(t, tc.wantEOF, errors.Is(err, io.EOF), "got %v", err)
			require.Equal(t, tc.wantErr, err != nil && !tc.wantEOF, "got %v", err)
			if err == nil {
				require.Equal(t, tc.want, string(got.Result))
			}
		})
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

func TestIndexParamsFileInputs(t *testing.T) {
	first := sdk.FileRef{Path: "a.go", Hash: "a"}
	second := sdk.FileRef{Path: "b.go", Hash: "b"}
	params := sdk.IndexParams{Inputs: []sdk.IndexInput{
		{Kind: "file", File: &first},
		{Kind: "regions", Regions: &sdk.EmbeddedFile{Path: "c.md"}},
		{Kind: "file", File: &second},
	}}

	require.Equal(t, []sdk.FileRef{first, second}, params.FileInputs())
}
