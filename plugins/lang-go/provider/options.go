package provider

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// options are the `[languages.go]` settings of lighthouse.toml.
type options struct {
	// Tags are build tags, as in `go build -tags`.
	Tags []string `json:"tags"`
	// Env adds environment variables for the go command, such as GOOS and
	// GOARCH. GOTOOLCHAIN defaults to local so analysis never downloads a
	// toolchain; set it here to allow that.
	Env map[string]string `json:"env"`
	// Go is the go binary to analyze with: an absolute path, or a name looked
	// up on PATH. By default the `go` found on PATH, which is the project's own
	// toolchain when a version manager shim is installed.
	Go string `json:"go"`
	// SmallInterfaces also reports `implements` edges for interfaces with a
	// single method, which nearly every type satisfies by accident.
	SmallInterfaces bool `json:"small_interfaces"`

	// bin is the absolute path of the go binary.
	bin string
}

// newOptions decodes the options of the Go language, rejecting unknown keys,
// and resolves the go binary.
func newOptions(raw map[string]json.RawMessage) (options, error) {
	var o options
	if data, ok := raw[language]; ok {
		dec := json.NewDecoder(bytes.NewReader(data))
		dec.DisallowUnknownFields()
		if err := dec.Decode(&o); err != nil {
			return options{}, fmt.Errorf("invalid [languages.%s] options: %w", language, err)
		}
	}
	name := o.Go
	if name == "" {
		name = "go"
	}
	bin, err := exec.LookPath(name)
	if err != nil {
		return options{}, fmt.Errorf("go command not found: %w", err)
	}
	if o.bin, err = filepath.Abs(bin); err != nil {
		return options{}, err
	}
	return o, nil
}

// environ is the environment of the go command: this process's with the
// configured additions, GOTOOLCHAIN=local unless configured, and the chosen
// binary's directory first on PATH when `go` is set.
func (o options) environ() []string {
	env := os.Environ()
	if o.Go != "" {
		env = append(env, "PATH="+filepath.Dir(o.bin)+string(os.PathListSeparator)+os.Getenv("PATH"))
	}
	if _, set := o.Env["GOTOOLCHAIN"]; !set {
		env = append(env, "GOTOOLCHAIN=local")
	}
	for k, v := range o.Env {
		env = append(env, k+"="+v)
	}
	return env
}

func (o options) buildFlags() []string {
	if len(o.Tags) == 0 {
		return nil
	}
	return []string{"-tags=" + strings.Join(o.Tags, ",")}
}

// load runs fn with the chosen go binary first on this process's PATH:
// go/packages resolves `go` there and cannot be told a binary.
func (o options) load(fn func()) {
	if o.Go == "" {
		fn()
		return
	}
	path := os.Getenv("PATH")
	_ = os.Setenv("PATH", filepath.Dir(o.bin)+string(os.PathListSeparator)+path)
	defer func() { _ = os.Setenv("PATH", path) }()
	fn()
}
