package provider

import (
	_ "embed"
	"go/build/constraint"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"
)

// stemFact is the name of the file fact that states a file's stem.
const stemFact = "stem"

// platformsText lists the GOOS and GOARCH names go/build treats as build
// constraints in a file name.
//
//go:embed platforms.txt
var platformsText string

var knownOS, knownArch = platforms()

// stemFacts states the stem of the file at rel when it differs from the
// file's name: every other file's stem is its name.
func (r *run) stemFacts(rel string) map[string]string {
	name := path.Base(rel)
	stem := r.variantStem(rel)
	if stem == strings.TrimSuffix(name, ".go") {
		return nil
	}
	return map[string]string{stemFact: stem}
}

// variantStem is the stem of a Go file: its name without the build constraint
// the name carries. A source file that a `//go:build` line constrains and
// whose name only adds a word (`enter_other.go`) shares the stem of another
// constrained file of its directory (`enter_arm64.go`, `enter.go`), so the
// variants of one source file have one stem.
func (r *run) variantStem(rel string) string {
	name := path.Base(rel)
	base := strings.TrimSuffix(name, ".go")
	stem := fileStem(name)
	if !strings.HasSuffix(name, ".go") || stem != base || strings.HasSuffix(base, "_test") || !strings.Contains(base, "_") || !r.constrained(rel) {
		return stem
	}
	prefix := base[:strings.LastIndex(base, "_")]
	if slices.Contains(r.constrainedStems(path.Dir(rel), name), prefix) {
		return prefix
	}
	return stem
}

// constrainedStems are the stems of the source files of dir other than the one
// named skip that a build constraint limits, by their names or by a
// `//go:build` line.
func (r *run) constrainedStems(dir, skip string) []string {
	entries, _ := os.ReadDir(filepath.Join(r.root, filepath.FromSlash(dir)))
	var stems []string
	for _, e := range entries {
		name := e.Name()
		if name == skip || e.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		stem := fileStem(name)
		if stem != strings.TrimSuffix(name, ".go") || r.constrained(path.Join(dir, name)) {
			stems = append(stems, stem)
		}
	}
	return stems
}

// constrained reports whether the file at rel starts with a `//go:build` line.
func (r *run) constrained(rel string) bool {
	abs := filepath.Join(r.root, filepath.FromSlash(rel))
	src, ok := r.overlays[abs]
	if !ok {
		src, _ = os.ReadFile(abs)
	}
	return hasBuildLine(src)
}

// hasBuildLine reports whether a `//go:build` line comes before the package
// clause of src.
func hasBuildLine(src []byte) bool {
	inBlock := false
	for line := range strings.Lines(string(src)) {
		line = strings.TrimSpace(line)
		switch {
		case inBlock:
			inBlock = !strings.Contains(line, "*/")
		case line == "":
		case constraint.IsGoBuild(line):
			return true
		case strings.HasPrefix(line, "//"):
		case strings.HasPrefix(line, "/*"):
			inBlock = !strings.Contains(line, "*/")
		default:
			return false
		}
	}
	return false
}

// fileStem is the name of a Go file without its extension and without the
// build constraint go/build reads from the name (`name_GOOS_GOARCH`,
// `name_GOOS` or `name_GOARCH`, before `_test`): `machine_arm64_test.go` is
// `machine_test`, `machine_linux_amd64.go` is `machine`. The `_test` suffix
// stays, so a test file's stem still tells it from its source file's.
func fileStem(name string) string {
	stem := strings.TrimSuffix(name, ".go")
	body, test := strings.CutSuffix(stem, "_test")
	// go/build ignores the first element: `linux.go` is no constraint.
	first, rest, ok := strings.Cut(body, "_")
	if !ok {
		return stem
	}
	parts := strings.Split(rest, "_")
	n := len(parts)
	switch {
	case n >= 2 && knownOS[parts[n-2]] && knownArch[parts[n-1]]:
		parts = parts[:n-2]
	case knownOS[parts[n-1]] || knownArch[parts[n-1]]:
		parts = parts[:n-1]
	}
	body = strings.Join(append([]string{first}, parts...), "_")
	if test {
		return body + "_test"
	}
	return body
}

// platforms returns the known GOOS and GOARCH names.
func platforms() (osNames, archNames map[string]bool) {
	osNames, archNames = map[string]bool{}, map[string]bool{}
	for line := range strings.SplitSeq(platformsText, "\n") {
		kind, names, _ := strings.Cut(line, " ")
		target, ok := map[string]map[string]bool{"os": osNames, "arch": archNames}[kind]
		if !ok {
			continue
		}
		for name := range strings.FieldsSeq(names) {
			target[name] = true
		}
	}
	return osNames, archNames
}
