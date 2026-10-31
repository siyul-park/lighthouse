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
