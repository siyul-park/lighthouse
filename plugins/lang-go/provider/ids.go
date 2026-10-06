package provider

import (
	"go/ast"
	"go/token"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

const (
	kindFunction  = "function"
	kindMethod    = "method"
	kindType      = "type"
	kindField     = "field"
	kindConst     = "const"
	kindVar       = "var"
	kindInterface = "interface"
	kindTest      = "test"

	edgeCalls      = "calls"
	edgeReferences = "references"
	edgeImports    = "imports"
	edgeContains   = "contains"
	edgeImplements = "implements"

	resolutionSemantic = "semantic"

	visibilityPublic   = "public"
	visibilityPrivate  = "private"
	visibilityInternal = "internal"

	roleTestHelper = "test-helper"
	roleFixture    = "fixture"

	styleTable    = "table"
	styleScenario = "scenario"
)

var testEntryPrefixes = []string{"Test", "Benchmark", "Fuzz", "Example"}

// symbolID is the project-stable id: module::owner::name#kind.
func symbolID(kind, module string, parts ...string) string {
	return target(module, parts...) + "#" + kind
}

// target is a kind-less symbol id: module::owner::name.
func target(module string, parts ...string) string {
	return module + "::" + strings.Join(parts, "::")
}

// span covers from..to inside from's file. After a syntax error the parser can
// leave an end past the end of the file, which the file set would resolve to
// whichever file follows.
func span(fset *token.FileSet, from, to token.Pos) sdk.Span {
	if f := fset.File(from); f != nil {
		to = min(to, token.Pos(f.Base()+f.Size()))
	}
	to = max(to, from)
	return sdk.Span{Start: position(fset, from), End: position(fset, to)}
}

func position(fset *token.FileSet, pos token.Pos) sdk.Position {
	p := fset.Position(pos)
	return sdk.Position{Line: p.Line, Col: p.Column}
}

func docText(groups ...*ast.CommentGroup) string {
	for _, g := range groups {
		if g != nil {
			if text := strings.TrimSpace(g.Text()); text != "" {
				return text
			}
		}
	}
	return ""
}

// isTestEntry reports a name of go test's entry-point form: one of the
// prefixes not followed by a lowercase letter.
func isTestEntry(name string, prefixes ...string) bool {
	for _, prefix := range prefixes {
		if rest, ok := strings.CutPrefix(name, prefix); ok {
			if r, _ := utf8.DecodeRuneInString(rest); !unicode.IsLower(r) {
				return true
			}
		}
	}
	return false
}
