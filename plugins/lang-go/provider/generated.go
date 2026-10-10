package provider

import (
	"go/ast"
	"strings"
)

// isGenerated reports whether a Go file is generated code. The standard marker
// (`// Code generated ... DO NOT EDIT.` on one line) is go/ast's; code
// generators that word it differently, such as protoc-gen-gogo's three-line
// header, also say "Code generated" and "DO NOT EDIT" in the comments above
// the package clause.
func isGenerated(file *ast.File) bool {
	if ast.IsGenerated(file) {
		return true
	}
	var header strings.Builder
	for _, group := range file.Comments {
		if group.Pos() > file.Package {
			break
		}
		header.WriteString(group.Text())
	}
	text := strings.ToLower(header.String())
	return strings.Contains(text, "code generated") && strings.Contains(text, "do not edit")
}
