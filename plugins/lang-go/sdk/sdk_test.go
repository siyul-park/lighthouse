package sdk_test

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

type handler struct {
	initialize func(sdk.InitializeParams) (sdk.InitializeResult, error)
	index      func(sdk.IndexParams) (sdk.IndexResult, error)
}

func (h handler) Initialize(p sdk.InitializeParams) (sdk.InitializeResult, error) {
	return h.initialize(p)
}

func (h handler) Index(p sdk.IndexParams) (sdk.IndexResult, error) { return h.index(p) }

func frame(t *testing.T, m sdk.Message) string {
	t.Helper()
	var b bytes.Buffer
	if err := sdk.WriteMessage(&b, &m); err != nil {
		t.Fatal(err)
	}
	return b.String()
}

func request(t *testing.T, id int, method string, params any) string {
	t.Helper()
	raw, err := json.Marshal(params)
	if err != nil {
		t.Fatal(err)
	}
	rawID := json.RawMessage(string(rune('0' + id)))
	return frame(t, sdk.Message{ID: &rawID, Method: method, Params: raw})
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
		if err != nil {
			t.Fatal(err)
		}
		all = append(all, *m)
	}
}

func TestFramingRoundTripsAndCountsBytes(t *testing.T) {
	id := json.RawMessage("7")
	in := sdk.Message{ID: &id, Result: json.RawMessage(`"日本語"`)}
	text := frame(t, in)
	header, body, _ := strings.Cut(text, "\r\n\r\n")
	if want := "Content-Length: " + itoa(len(body)); header != want {
		t.Fatalf("header %q, want %q", header, want)
	}
	got, err := sdk.ReadMessage(bufio.NewReader(strings.NewReader(text)))
	if err != nil {
		t.Fatal(err)
	}
	if string(got.Result) != `"日本語"` || string(*got.ID) != "7" || got.JSONRPC != "2.0" {
		t.Fatalf("round trip lost data: %+v", got)
	}
}

func itoa(n int) string {
	b, _ := json.Marshal(n)
	return string(b)
}

func TestReadMessageRejectsMalformedFrames(t *testing.T) {
	for name, input := range map[string]string{
		"no length":     "Content-Type: x\r\n\r\n{}",
		"bad length":    "Content-Length: nope\r\n\r\n",
		"short body":    "Content-Length: 9\r\n\r\n{}",
		"bad json":      "Content-Length: 2\r\n\r\n{]",
		"header no end": "Content-Length: 2\r\n",
		"too large":     "Content-Length: 999999999999\r\n\r\n",
	} {
		if _, err := sdk.ReadMessage(bufio.NewReader(strings.NewReader(input))); err == nil || errors.Is(err, io.EOF) {
			t.Errorf("%s: got %v, want a malformed-frame error", name, err)
		}
	}
	if _, err := sdk.ReadMessage(bufio.NewReader(strings.NewReader(""))); !errors.Is(err, io.EOF) {
		t.Errorf("empty input: got %v, want io.EOF", err)
	}
}

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

func TestServeAnswersRequestsAndStopsAfterShutdownAndExit(t *testing.T) {
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
	if err := sdk.Serve(strings.NewReader(in.String()), &out, newHandler()); err != nil {
		t.Fatalf("Serve: %v", err)
	}
	got := replies(t, out.String())
	if len(got) != 7 {
		t.Fatalf("got %d replies, want 7", len(got))
	}
	var init sdk.InitializeResult
	if err := json.Unmarshal(got[0].Result, &init); err != nil || init.Languages[0].ID != "/r" {
		t.Errorf("initialize result %s (%v)", got[0].Result, err)
	}
	var index sdk.IndexResult
	if err := json.Unmarshal(got[1].Result, &index); err != nil || index.Notices[0] != "/r" {
		t.Errorf("index result %s (%v)", got[1].Result, err)
	}
	if string(got[1].Result) != `{"fragments":[],"notices":["/r"],"incomplete":[]}` {
		t.Errorf("empty lists must encode as [], got %s", got[1].Result)
	}
	for i, code := range map[int]int{2: -32601, 3: -32603, 4: -32603, 5: -32602} {
		if got[i].Error == nil || got[i].Error.Code != code {
			t.Errorf("reply %d: got %+v, want error %d", i, got[i].Error, code)
		}
	}
	if !strings.Contains(got[3].Error.Message, "panic: boom") {
		t.Errorf("panic not reported: %q", got[3].Error.Message)
	}
	if string(got[6].Result) != "null" {
		t.Errorf("shutdown result %s, want null", got[6].Result)
	}
}

func TestServeReportsAnInputThatEndsOrExitsTooEarly(t *testing.T) {
	var out bytes.Buffer
	if err := sdk.Serve(strings.NewReader(""), &out, newHandler()); err == nil {
		t.Error("EOF before exit must be an error")
	}
	exit := frame(t, sdk.Message{Method: "exit"})
	if err := sdk.Serve(strings.NewReader(exit), &out, newHandler()); err == nil {
		t.Error("exit without shutdown must be an error")
	}
}

func TestServeAnswersGarbageWithAParseErrorAndStops(t *testing.T) {
	var out bytes.Buffer
	err := sdk.Serve(strings.NewReader("Content-Length: 2\r\n\r\n{]"), &out, newHandler())
	if err == nil {
		t.Fatal("garbage must end the run")
	}
	got := replies(t, out.String())
	if len(got) != 1 || got[0].Error == nil || got[0].Error.Code != -32700 {
		t.Fatalf("got %+v, want one parse error", got)
	}
}
