package provider_test

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

func TestFileStem(t *testing.T) {
	t.Run("drops the build constraint go/build reads from the name", func(t *testing.T) {
		found := stems(t, "linux", "arm64",
			"machine.go", "machine_test.go", "machine_arm64.go", "machine_arm64_test.go",
			"machine_linux_arm64.go", "machine_linux.go", "linux.go", "read_only.go",
			"machine_arm64_linux.go")

		require.Equal(t, map[string]string{
			"machine.go":             "",
			"machine_test.go":        "",
			"machine_arm64.go":       "machine",
			"machine_arm64_test.go":  "machine_test",
			"machine_linux_arm64.go": "machine",
			"machine_linux.go":       "machine",
			"linux.go":               "",
			"read_only.go":           "",
			"machine_arm64_linux.go": "machine_arm64",
		}, found)
	})

	t.Run("knows every platform of go tool dist list", func(t *testing.T) {
		out, err := exec.Command("go", "tool", "dist", "list").Output()
		if err != nil {
			t.Skip("go tool dist list is unavailable")
		}
		for platform := range strings.FieldsSeq(string(out)) {
			goos, goarch, _ := strings.Cut(platform, "/")
			name := "x_" + goos + "_" + goarch + ".go"
			require.Equal(t, "x", stems(t, goos, goarch, name)[name], platform)
		}
	})
}

// stems indexes the files, named by their constraint-free forms, built for
// goos and goarch, and returns the stem each file states ("" when it states
// none).
func stems(t *testing.T, goos, goarch string, names ...string) map[string]string {
	t.Helper()
	root := t.TempDir()
	write := func(name, body string) {
		require.NoError(t, os.WriteFile(filepath.Join(root, name), []byte(body), 0o644))
	}
	write("go.mod", "module example.com/p\n\ngo 1.22\n")
	var refs []sdk.FileRef
	for _, name := range names {
		write(name, "package p\n")
		refs = append(refs, sdk.FileRef{Path: name})
	}
	options, err := json.Marshal(map[string]any{"env": map[string]string{"GOOS": goos, "GOARCH": goarch}})
	require.NoError(t, err)
	result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Inputs:   fileInputs(refs),
		Context:  sdk.Context{Options: map[string]json.RawMessage{"go": options}},
	})
	require.NoError(t, err)
	found := map[string]string{}
	for _, fragment := range result.Fragments {
		found[fragment.File.Path] = fragment.File.Facts["stem"]
	}
	return found
}
