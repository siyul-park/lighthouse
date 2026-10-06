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

type handler struct {
	initialize func(sdk.InitializeParams) (sdk.InitializeResult, error)
	index      func(sdk.IndexParams) (sdk.IndexResult, error)
}

func (h handler) Initialize(p sdk.InitializeParams) (sdk.InitializeResult, error) {
	return h.initialize(p)
}

func (h handler) Index(p sdk.IndexParams) (sdk.IndexResult, error) { return h.index(p) }

func newHandler() handler {
	return handler{
		initialize: func(p sdk.InitializeParams) (sdk.InitializeResult, error) {
			return sdk.InitializeResult{ID: "x", Version: "1", ProtocolVersion: sdk.ProtocolVersion, Languages: []sdk.Language{{ID: p.Root}}}, nil
		},
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
		in.WriteString(request(t, 1, "initialize", sdk.InitializeParams{Root: "/r", ProtocolVersion: "0.1"}))
		in.WriteString(request(t, 2, "index", sdk.IndexParams{Project: sdk.ProjectRef{Root: "/r"}, Language: "go"}))
		in.WriteString(request(t, 3, "nope", struct{}{}))
		in.WriteString(request(t, 4, "index", sdk.IndexParams{Language: "panic"}))
		in.WriteString(request(t, 5, "index", sdk.IndexParams{Language: "fail"}))
		in.WriteString(request(t, 6, "index", "not an object"))
		in.WriteString(request(t, 7, "shutdown", nil))
		in.WriteString(frame(t, sdk.Message{Method: "exit"}))
		var out bytes.Buffer

		err := sdk.Serve(strings.NewReader(in.String()), &out, newHandler())

		require.NoError(t, err)
		got := replies(t, out.String())
		require.Len(t, got, 7)
		var init sdk.InitializeResult
		require.NoError(t, json.Unmarshal(got[0].Result, &init))
		require.Equal(t, "/r", init.Languages[0].ID)
		require.JSONEq(t, `{"fragments":[],"notices":["/r"],"incomplete":[]}`, string(got[1].Result))
		for i, code := range map[int]int{2: -32601, 3: -32603, 4: -32603, 5: -32602} {
			require.NotNil(t, got[i].Error, "reply %d", i)
			require.Equal(t, code, got[i].Error.Code, "reply %d", i)
		}
		require.Contains(t, got[3].Error.Message, "panic: boom")
		require.Equal(t, "null", string(got[6].Result))
	})

	t.Run("reports an input that ends or exits too early", func(t *testing.T) {
		var out bytes.Buffer

		endErr := sdk.Serve(strings.NewReader(""), &out, newHandler())
		exitErr := sdk.Serve(strings.NewReader(frame(t, sdk.Message{Method: "exit"})), &out, newHandler())

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
