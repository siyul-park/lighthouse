package provider

import (
	"go/ast"
	"go/token"
	"go/types"
	"path"
	"strings"

	"golang.org/x/tools/go/packages"
)

// unit is one requested file together with the package that type-checked it.
type unit struct {
	rel  string
	file *ast.File
	pkg  *packages.Package
	fset *token.FileSet
	src  []byte
	res  *resolver

	module   string
	testOf   string
	main     bool
	internal bool
	test     bool
}

func newUnit(rel string, file *ast.File, pkg *packages.Package, src []byte, res *resolver) *unit {
	dir := path.Dir(rel)
	u := &unit{
		rel:      rel,
		file:     file,
		pkg:      pkg,
		fset:     pkg.Fset,
		src:      src,
		res:      res,
		module:   dir,
		main:     file.Name.Name == "main",
		internal: hasComponent(dir, "internal"),
		test:     strings.HasSuffix(rel, "_test.go"),
	}
	if u.test && strings.HasSuffix(file.Name.Name, "_test") {
		u.module = dir + moduleSuffixTest
		u.testOf = dir
	}
	return u
}

func hasComponent(dir, name string) bool {
	for _, part := range strings.Split(dir, "/") {
		if part == name {
			return true
		}
	}
	return false
}

func (u *unit) info() *types.Info { return u.pkg.TypesInfo }

// visibility follows Go's capitalization, tightened by where the symbol
// lives: nothing of a main package is importable and symbols under an
// internal directory are exported only inside the project.
func (u *unit) visibility(name string) string {
	switch {
	case u.main || !ast.IsExported(name):
		return visibilityPrivate
	case u.internal:
		return visibilityInternal
	default:
		return visibilityPublic
	}
}
