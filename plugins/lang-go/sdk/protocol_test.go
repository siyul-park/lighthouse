package sdk_test

import (
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

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

func TestWorkspaceFolderPath(t *testing.T) {
	t.Run("names the directory of a file URI", func(t *testing.T) {
		path, err := sdk.WorkspaceFolder{URI: "file:///r%20x/"}.Path()

		require.NoError(t, err)
		require.Equal(t, "/r x", path)
	})

	t.Run("refuses a URI that is not a file URI", func(t *testing.T) {
		_, err := sdk.WorkspaceFolder{URI: "http://x/"}.Path()

		require.Error(t, err)
	})
}
