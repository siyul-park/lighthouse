package provider

import (
	_ "embed"
	"path"
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
func stemFacts(rel string) map[string]string {
	name := path.Base(rel)
	stem := fileStem(name)
	if stem == strings.TrimSuffix(name, ".go") {
		return nil
	}
	return map[string]string{stemFact: stem}
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
