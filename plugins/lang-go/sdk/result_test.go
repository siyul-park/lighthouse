package sdk_test

import (
	"encoding/json"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

func TestIndexResult_MarshalJSON(t *testing.T) {
	t.Run("merges encoded fragments in path order", func(t *testing.T) {
		result := sdk.IndexResult{
			Fragments: []sdk.Fragment{{File: sdk.FileInfo{Path: "b.go"}}, {File: sdk.FileInfo{Path: "d.go"}}},
			Encoded:   []sdk.EncodedFragment{{Path: "a.go", JSON: json.RawMessage(`{"x":1}`)}, {Path: "c.go", JSON: json.RawMessage(`{"x":2}`)}},
			Notices:   []string{},
		}

		raw, err := json.Marshal(result)

		require.NoError(t, err)
		var back struct {
			Fragments []struct {
				X    int `json:"x"`
				File struct {
					Path string `json:"path"`
				} `json:"file"`
			} `json:"fragments"`
		}
		require.NoError(t, json.Unmarshal(raw, &back))
		require.Len(t, back.Fragments, 4)
		require.Equal(t, 1, back.Fragments[0].X)
		require.Equal(t, "b.go", back.Fragments[1].File.Path)
		require.Equal(t, 2, back.Fragments[2].X)
		require.Equal(t, "d.go", back.Fragments[3].File.Path)
	})

	t.Run("encodes no fragments as null like any empty list", func(t *testing.T) {
		raw, err := json.Marshal(sdk.IndexResult{})

		require.NoError(t, err)
		require.Contains(t, string(raw), `"fragments":null`)
	})
}
