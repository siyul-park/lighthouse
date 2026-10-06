package provider

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestIsTestNameFollowsGoTest(t *testing.T) {
	for name, want := range map[string]bool{
		"Test":         true,
		"TestAdd":      true,
		"Test_add":     true,
		"Testing":      false,
		"BenchmarkX":   true,
		"Benchmarking": false,
		"FuzzParse":    true,
		"ExampleRun":   true,
		"Examples":     false,
		"helper":       false,
	} {
		if got := isTestName(name); got != want {
			t.Errorf("isTestName(%q) = %v, want %v", name, got, want)
		}
	}
	if isTestCaseName("BenchmarkX") || !isTestCaseName("TestX") {
		t.Error("only Test entry points carry a test case")
	}
}

func TestIgnoredPathFollowsTheGoToolsRules(t *testing.T) {
	for path, want := range map[string]bool{
		"a/b.go":              false,
		"testdata/x.go":       true,
		"a/testdata/b/x.go":   true,
		"vendor/dep/dep.go":   true,
		"a/_hidden/x.go":      true,
		"a/.git/x.go":         true,
		"a/_x.go":             true,
		"internal/testing.go": false,
	} {
		if got := ignoredPath(path); got != want {
			t.Errorf("ignoredPath(%q) = %v, want %v", path, got, want)
		}
	}
}

func TestPositionFileStripsLineAndColumn(t *testing.T) {
	for in, want := range map[string]string{
		"/a/b.go:3:9": "/a/b.go",
		"/a/b.go:3":   "/a/b.go",
		"/a/b.go":     "/a/b.go",
		"":            "",
		"-":           "-",
	} {
		if got := positionFile(in); got != want {
			t.Errorf("positionFile(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestLocalModulePlacesImportPathsUnderTheProjectRoot(t *testing.T) {
	root := &goModule{path: "example.com/app", dir: "."}
	nested := &goModule{path: "example.com/tools", dir: "tools"}
	for _, c := range []struct {
		m    *goModule
		in   string
		want string
		ok   bool
	}{
		{root, "example.com/app", ".", true},
		{root, "example.com/app/internal/x", "internal/x", true},
		{root, "example.com/application", "", false},
		{root, "fmt", "", false},
		{nested, "example.com/tools", "tools", true},
		{nested, "example.com/tools/cmd", "tools/cmd", true},
		{nil, "anything", "", false},
	} {
		got, ok := c.m.locate(c.in)
		if got != c.want || ok != c.ok {
			t.Errorf("locate(%q) = %q, %v, want %q, %v", c.in, got, ok, c.want, c.ok)
		}
	}
}

func TestParseOptionsAcceptsKnownKeysOnly(t *testing.T) {
	if _, err := parseOptions(nil, "go"); err != nil {
		t.Fatalf("absent options: %v", err)
	}
	good := map[string]json.RawMessage{"go": json.RawMessage(`{"tags":["a","b"],"env":{"GOOS":"linux"}}`)}
	o, err := parseOptions(good, "go")
	if err != nil {
		t.Fatal(err)
	}
	if flags := o.buildFlags(); len(flags) != 1 || flags[0] != "-tags=a,b" {
		t.Errorf("buildFlags = %v", flags)
	}
	bad := map[string]json.RawMessage{"go": json.RawMessage(`{"tagz":[]}`)}
	if _, err := parseOptions(bad, "go"); err == nil || !strings.Contains(err.Error(), "tagz") {
		t.Errorf("unknown key accepted: %v", err)
	}
	other := map[string]json.RawMessage{"python": json.RawMessage(`{"anything":1}`)}
	if _, err := parseOptions(other, "go"); err != nil {
		t.Errorf("another language's options must be ignored: %v", err)
	}
}
