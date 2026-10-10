package provider

import (
	"crypto/sha256"
	"encoding/hex"
	"go/ast"
	"go/parser"
	"go/token"
	"go/types"
	"sort"
	"strconv"
)

// fileFacts are what the unit cache needs to know about one Go source file,
// derived from its text alone.
type fileFacts struct {
	// Imports are the import paths of the file.
	Imports []string `json:"imports"`
	// API digests the text of the file without the bodies of its functions and
	// methods. An edit inside a body leaves it unchanged, and with it the key of
	// every package that depends on this one (early cutoff).
	API string `json:"api"`
	// Interfaces digests the type declarations that may declare an interface;
	// empty when there are none. `implements` edges read every project
	// interface, so any change here changes every key.
	Interfaces string `json:"interfaces"`
}

// digest hashes parts without letting their boundaries blur.
func digest(parts ...string) string {
	h := sha256.New()
	for _, p := range parts {
		h.Write([]byte(strconv.Itoa(len(p))))
		h.Write([]byte{':'})
		h.Write([]byte(p))
	}
	return hex.EncodeToString(h.Sum(nil))
}

// contentHash is the SHA-256 the host sends for a file, in lowercase hex.
func contentHash(src []byte) string {
	sum := sha256.Sum256(src)
	return hex.EncodeToString(sum[:])
}

// factsOf reads the facts out of source text. A file that does not parse is
// treated as all API: any change to it changes every dependent.
func factsOf(src []byte) fileFacts {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, "", src, parser.SkipObjectResolution)
	if file == nil {
		whole := contentHash(src)
		return fileFacts{API: whole, Interfaces: whole}
	}
	facts := fileFacts{Imports: importsOf(file)}
	tf := fset.File(file.Pos())
	if err != nil || tf == nil {
		whole := contentHash(src)
		facts.API, facts.Interfaces = whole, whole
		return facts
	}
	offset := func(p token.Pos) int { return tf.Offset(p) }
	facts.API = apiDigest(src, file, offset)
	facts.Interfaces = interfaceDigest(src, file, offset)
	return facts
}

func importsOf(file *ast.File) []string {
	seen := map[string]bool{}
	var out []string
	for _, spec := range file.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil || seen[path] {
			continue
		}
		seen[path] = true
		out = append(out, path)
	}
	sort.Strings(out)
	return out
}

// apiDigest hashes the text between the function bodies of a file.
func apiDigest(src []byte, file *ast.File, offset func(token.Pos) int) string {
	h := sha256.New()
	at := 0
	for _, decl := range file.Decls {
		fn, ok := decl.(*ast.FuncDecl)
		if !ok || fn.Body == nil {
			continue
		}
		start, end := offset(fn.Body.Lbrace), offset(fn.Body.Rbrace)+1
		if start < at || end > len(src) {
			return contentHash(src)
		}
		h.Write(src[at:start])
		at = end
	}
	h.Write(src[at:])
	return hex.EncodeToString(h.Sum(nil))
}

// interfaceDigest hashes the type specs whose type might be an interface:
// literal interfaces, and names or instantiations that could stand for one.
func interfaceDigest(src []byte, file *ast.File, offset func(token.Pos) int) string {
	h := sha256.New()
	found := false
	for _, decl := range file.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.TYPE {
			continue
		}
		for _, spec := range gen.Specs {
			ts, ok := spec.(*ast.TypeSpec)
			if !ok || !mayBeInterface(ts.Type) {
				continue
			}
			start, end := offset(ts.Pos()), offset(ts.End())
			if start < 0 || end > len(src) || start > end {
				return contentHash(src)
			}
			found = true
			h.Write(src[start:end])
			h.Write([]byte{0})
		}
	}
	if !found {
		return ""
	}
	return hex.EncodeToString(h.Sum(nil))
}

func mayBeInterface(expr ast.Expr) bool {
	switch t := expr.(type) {
	case *ast.Ident:
		if tn, ok := types.Universe.Lookup(t.Name).(*types.TypeName); ok {
			_, basic := tn.Type().Underlying().(*types.Basic)
			return !basic
		}
		return true
	case *ast.InterfaceType, *ast.SelectorExpr, *ast.IndexExpr, *ast.IndexListExpr, *ast.ParenExpr:
		return true
	default:
		return false
	}
}
