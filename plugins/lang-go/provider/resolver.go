package provider

import (
	"go/types"
	"path/filepath"
	"slices"
	"strings"

	"golang.org/x/tools/go/packages"
)

// resolver turns type-checked objects into Lighthouse symbol targets.
type resolver struct {
	// modules maps a package path to the module path of its directory.
	modules map[string]string
	// project holds the package paths that live inside the project.
	project map[string]bool
	// local places import paths of packages that did not load, such as a
	// directory that fails to build, under the module that would hold them.
	local *goModule
}

// goModule is the module of a go.mod: its import path and its directory
// relative to the project root.
type goModule struct {
	path string
	dir  string
}

// moduleSuffixTest marks the module of an external test package.
const moduleSuffixTest = "[test]"

// newResolver maps every package reachable from roots. Packages inside the
// project root get their directory, relative to it, as module path; all
// others (the standard library, dependencies, vendored code) keep their
// import path.
func newResolver(root string, roots []*packages.Package, local *goModule) *resolver {
	r := &resolver{modules: map[string]string{}, project: map[string]bool{}, local: local}
	packages.Visit(roots, nil, func(p *packages.Package) {
		if module, inside := moduleOf(root, p); module != "" && r.modules[p.PkgPath] == "" {
			r.modules[p.PkgPath] = module
			r.project[p.PkgPath] = inside
		}
	})
	return r
}

// object is the target of a package-level function, variable, constant or
// type, or of a method. Fields need their holder and go through field.
func (r *resolver) object(obj types.Object) (string, bool) {
	if obj.Pkg() == nil || !r.inProject(obj.Pkg()) {
		return "", false
	}
	switch obj := obj.(type) {
	case *types.Func:
		return r.function(obj)
	case *types.Var:
		if obj.IsField() || !declared(obj) {
			return "", false
		}
	case *types.Const, *types.TypeName:
		if !declared(obj) {
			return "", false
		}
	default:
		return "", false
	}
	return target(r.module(obj.Pkg()), obj.Name()), true
}

func (r *resolver) function(fn *types.Func) (string, bool) {
	fn = fn.Origin()
	if fn.Pkg() == nil || !r.inProject(fn.Pkg()) {
		return "", false
	}
	sig, ok := fn.Type().(*types.Signature)
	if !ok {
		return "", false
	}
	if sig.Recv() == nil {
		if !declared(fn) {
			return "", false
		}
		return target(r.module(fn.Pkg()), fn.Name()), true
	}
	owner, ok := namedType(sig.Recv().Type())
	if !ok || !r.inProject(owner.Pkg()) {
		return "", false
	}
	return target(r.module(owner.Pkg()), owner.Name(), fn.Name()), true
}

// field is the target of a field of the named type holder.
func (r *resolver) field(holder *types.TypeName, field types.Object) (string, bool) {
	if !r.inProject(holder.Pkg()) {
		return "", false
	}
	return target(r.module(holder.Pkg()), holder.Name(), field.Name()), true
}

// inProject reports whether a package lives inside the project, including one
// that failed to load but belongs to the project's module.
func (r *resolver) inProject(p *types.Package) bool {
	if r.project[p.Path()] {
		return true
	}
	_, ok := r.local.locate(p.Path())
	return ok && r.modules[p.Path()] == ""
}

func (r *resolver) module(p *types.Package) string {
	return r.importPath(p.Path())
}

func (r *resolver) importPath(path string) string {
	if m := r.modules[path]; m != "" {
		return m
	}
	if m, ok := r.local.locate(path); ok {
		return m
	}
	return path
}

func (m *goModule) locate(importPath string) (string, bool) {
	if m == nil {
		return "", false
	}
	rest, ok := strings.CutPrefix(importPath, m.path)
	if !ok || (rest != "" && !strings.HasPrefix(rest, "/")) {
		return "", false
	}
	rest = strings.TrimPrefix(rest, "/")
	switch {
	case m.dir == "." && rest == "":
		return ".", true
	case m.dir == ".":
		return rest, true
	case rest == "":
		return m.dir, true
	default:
		return m.dir + "/" + rest, true
	}
}

// moduleOf is the module path of a package and whether it lives inside the
// project: its directory relative to the root, or its import path when it is
// outside the root or vendored. A package without files (one that failed to
// load) has no module here.
func moduleOf(root string, p *packages.Package) (module string, inside bool) {
	if len(p.GoFiles) == 0 {
		return "", false
	}
	rel, err := filepath.Rel(root, filepath.Dir(p.GoFiles[0]))
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return p.PkgPath, false
	}
	rel = filepath.ToSlash(rel)
	for _, part := range strings.Split(rel, "/") {
		if part == "vendor" {
			return p.PkgPath, false
		}
	}
	if isExternalTest(p) {
		rel += moduleSuffixTest
	}
	return rel, true
}

// isExternalTest reports the package of `package x_test` files: a package
// named like its import path with a `_test` suffix whose files are all test
// files. The go command sets ForTest only when the directory also has
// in-package tests.
func isExternalTest(p *packages.Package) bool {
	if !strings.HasSuffix(p.PkgPath, "_test") || !strings.HasSuffix(p.Name, "_test") {
		return false
	}
	return len(p.GoFiles) > 0 && !slices.ContainsFunc(p.GoFiles, func(f string) bool {
		return !strings.HasSuffix(f, "_test.go")
	})
}

// holder is the named type that declares the field a selection ends in,
// following embedded fields; anonymous structs have none.
func holder(s *types.Selection) (*types.TypeName, bool) {
	t := s.Recv()
	index := s.Index()
	for _, i := range index[:len(index)-1] {
		st, ok := structOf(t)
		if !ok {
			return nil, false
		}
		t = st.Field(i).Type()
	}
	return namedType(t)
}

func structOf(t types.Type) (*types.Struct, bool) {
	t = types.Unalias(t)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	st, ok := t.Underlying().(*types.Struct)
	return st, ok
}

// namedType is the declared package-level type behind pointers and aliases.
func namedType(t types.Type) (*types.TypeName, bool) {
	t = types.Unalias(t)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	named, ok := t.(*types.Named)
	if !ok {
		return nil, false
	}
	obj := named.Origin().Obj()
	return obj, declared(obj)
}

// declared reports whether obj is a package-level object of a named package.
func declared(obj types.Object) bool {
	return obj.Pkg() != nil && obj.Parent() == obj.Pkg().Scope()
}
