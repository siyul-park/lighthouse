package provider

import (
	"go/ast"
	"strings"
)

// isGenerated reports whether a Go file is generated code. The standard marker
// (`// Code generated ... DO NOT EDIT.` on one line) is go/ast's. Code
// generators that word it differently, such as protoc-gen-gogo's three-line
// header, also start a line of a comment above the package clause with "Code
// generated" and say "DO NOT EDIT" on that line or a later line of the same
// comment. The package documentation is not a header: it may mention both.
func isGenerated(file *ast.File) bool {
	if ast.IsGenerated(file) {
		return true
	}
	for _, group := range file.Comments {
		if group.Pos() > file.Package {
			break
		}
		if group != file.Doc && marksGenerated(group.Text()) {
			return true
		}
	}
	return false
}

// marksGenerated reports whether a comment has a line that starts with "Code
// generated" and says "DO NOT EDIT" on it or on a later line.
func marksGenerated(text string) bool {
	started := false
	for line := range strings.SplitSeq(text, "\n") {
		started = started || strings.HasPrefix(line, "Code generated")
		if started && strings.Contains(strings.ToUpper(line), "DO NOT EDIT") {
			return true
		}
	}
	return false
}
