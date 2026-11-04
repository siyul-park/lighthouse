package provider

import (
	"go/ast"
	"go/types"
	"slices"
	"sync"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/tools/go/packages"
)

// interfaceSet is the interfaces the code of a package names, by method name,
// and those it satisfies without naming them (see wider).
type interfaceSet struct {
	once     sync.Once
	byMethod map[string][]*types.Interface
	wider    map[string][]*types.Interface
	// near is wider without the interfaces of project packages the package
	// does not reach through its imports.
	near map[string][]*types.Interface
	// embedders are the types of the package that embed a type, by the
	// embedded type: its methods are promoted into them.
	embedders map[*types.TypeName][]*types.Named
}

const resolutionSyntactic = "syntactic"

// roles: what a function is for, as far as the types say. A method whose
// signature an interface dictates is an implementation; a function without a
// receiver that returns a type of its package builds it.

// implementation reports whether the method satisfies an interface of the
// project or of a package the code can import, named by the package or not:
// some candidate interface has a method of the same name and the receiver
// type (or a pointer to it) implements the interface. far says that only an
// interface of a package of the project that the unit's package does not import
// does: what a request that loads the package alone does not see.
func (u *unit) implementation(d *ast.FuncDecl) (impl, far bool) {
	if d.Recv == nil {
		return false, false
	}
	fn, ok := u.info().Defs[d.Name].(*types.Func)
	if !ok {
		return false, false
	}
	recv := fn.Type().(*types.Signature).Recv()
	if recv == nil {
		return false, false
	}
	named, ok := receiverNamed(recv.Type())
	if !ok {
		return false, false
	}
	if overridesPromoted(named, u.pkg.Types, d.Name.Name) {
		return true, false
	}
	set := u.res.interfacesOf(u.pkg)
	name := d.Name.Name
	if satisfiesAny(named, set, slices.Concat(set.byMethod[name], set.near[name])) {
		return true, false
	}
	if satisfiesAny(named, set, set.wider[name]) {
		return true, true
	}
	return false, false
}

// constructs reports whether the function has no receiver and returns T, *T
// or (T, error) for a type T declared in its package.
func (u *unit) constructs(d *ast.FuncDecl) bool {
	if d.Recv != nil {
		return false
	}
	fn, ok := u.info().Defs[d.Name].(*types.Func)
	if !ok {
		return false
	}
	results := fn.Type().(*types.Signature).Results()
	switch results.Len() {
	case 1:
		return declaredIn(results.At(0).Type(), u.pkg.Types)
	case 2:
		return declaredIn(results.At(0).Type(), u.pkg.Types) && isError(results.At(1).Type())
	}
	return false
}

// configures names the type a functional-option builder configures: the
// function has no receiver and its only result is a func type, written out or
// named in the package, that takes T or *T, for a type T of the package, as its
// only parameter and returns nothing. When constructors of the package take
// that option type and build a type of the package, the builder configures
// that one instead (see builtBy). It is empty for any other function.
func (u *unit) configures(d *ast.FuncDecl) string {
	if d.Recv != nil {
		return ""
	}
	fn, ok := u.info().Defs[d.Name].(*types.Func)
	if !ok {
		return ""
	}
	results := fn.Type().(*types.Signature).Results()
	if results.Len() != 1 {
		return ""
	}
	result := types.Unalias(results.At(0).Type())
	if named, ok := result.(*types.Named); ok && named.Obj().Pkg() != u.pkg.Types {
		return ""
	}
	option, ok := result.Underlying().(*types.Signature)
	if !ok || option.Params().Len() != 1 || option.Results().Len() != 0 {
		return ""
	}
	owner, ok := namedType(option.Params().At(0).Type())
	if !ok || owner.Pkg() != u.pkg.Types {
		return ""
	}
	if built := u.res.constructorsOf(u.pkg).builtBy(result, u.fset.Position(d.Pos()).Filename); built != nil {
		owner = built
	}
	return target(u.res.module(owner.Pkg()), owner.Name())
}

// unresolved states what the fence of a document uses by name where the type
// checker knows nothing of the name: prose defines names, and a fence that
// uses them is a fragment. Each is a `syntactic` edge to the bare name, which
// the project settles when one symbol of the project is called so.
func (u *unit) unresolved() {
	from := sdk.Node{Module: u.module}
	info := u.info()
	known := func(id *ast.Ident) bool {
		return id.Name == "_" || id.Obj != nil || info.Uses[id] != nil || info.Defs[id] != nil
	}
	called := map[ast.Expr]bool{}
	skipped := map[*ast.Ident]bool{}
	ast.Inspect(u.file, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.CallExpr:
			called[n.Fun] = true
		case *ast.KeyValueExpr:
			if key, ok := n.Key.(*ast.Ident); ok {
				skipped[key] = true
			}
		case *ast.SelectorExpr:
			qualifier, ok := n.X.(*ast.Ident)
			if !ok || known(qualifier) {
				// The type checker settled what is selected from a value
				// or a package it knows.
				skipped[n.Sel] = true
				break
			}
			skipped[qualifier] = true
			if info.Uses[n.Sel] == nil && info.Selections[n] == nil {
				u.unresolvedUse(from, n.Sel, called[n])
			}
			skipped[n.Sel] = true
		}
		return true
	})
	ast.Inspect(u.file, func(n ast.Node) bool {
		if id, ok := n.(*ast.Ident); ok && !skipped[id] && !known(id) && id != u.file.Name {
			u.unresolvedUse(from, id, called[id])
		}
		return true
	})
}

// unresolvedUse emits the edge of one use of a name.
func (u *unit) unresolvedUse(from sdk.Node, id *ast.Ident, call bool) {
	kind := edgeReferences
	if call {
		kind = edgeCalls
	}
	site := span(u.fset, id.Pos(), id.End())
	u.edgeAs(kind, from, id.Name, &site, resolutionSyntactic)
}

// satisfiesAny reports whether the type, or a type of the package that
// embeds it, satisfies one of the interfaces.
func satisfiesAny(named *types.Named, set *interfaceSet, ifaces []*types.Interface) bool {
	for _, iface := range ifaces {
		if satisfies(named, iface) {
			return true
		}
		for _, holder := range set.embedders[named.Obj()] {
			if satisfies(holder, iface) {
				return true
			}
		}
	}
	return false
}

func collectInterfaces(pkg *packages.Package) map[string][]*types.Interface {
	found := newMethodIndex()
	add := func(t types.Type) {
		if iface, ok := candidate(t); ok {
			found.add(iface)
		}
	}
	for _, objects := range []map[*ast.Ident]types.Object{pkg.TypesInfo.Uses, pkg.TypesInfo.Defs} {
		for _, obj := range objects {
			if tn, ok := obj.(*types.TypeName); ok && tn.Type() != nil {
				add(tn.Type())
			}
		}
	}
	for _, tv := range pkg.TypesInfo.Types {
		if tv.Type != nil && !tv.IsType() {
			add(tv.Type)
		}
	}
	for _, file := range pkg.Syntax {
		ast.Inspect(file, func(n ast.Node) bool {
			if call, ok := n.(*ast.CallExpr); ok {
				addParameters(pkg.TypesInfo, call, add)
			}
			return true
		})
	}
	return found.byName
}

// collectEmbedders maps each type embedded in a struct type of the package
// (behind a pointer or not) to the struct types that embed it.
func collectEmbedders(pkg *packages.Package) map[*types.TypeName][]*types.Named {
	found := map[*types.TypeName][]*types.Named{}
	scope := pkg.Types.Scope()
	for _, name := range scope.Names() {
		tn, ok := scope.Lookup(name).(*types.TypeName)
		if !ok {
			continue
		}
		holder, ok := tn.Type().(*types.Named)
		if !ok || holder.TypeParams().Len() > 0 {
			continue
		}
		st, ok := holder.Underlying().(*types.Struct)
		if !ok {
			continue
		}
		for field := range st.Fields() {
			if embedded, ok := receiverNamed(field.Type()); ok && field.Embedded() {
				found[embedded.Obj()] = append(found[embedded.Obj()], holder)
			}
		}
	}
	return found
}

func isError(t types.Type) bool {
	return t != nil && types.Identical(t, errorType)
}

// receiverNamed is the named type behind an optional pointer.
func receiverNamed(t types.Type) (*types.Named, bool) {
	t = types.Unalias(t)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	named, ok := t.(*types.Named)
	return named, ok
}

// addParameters offers the parameter types of the function a call invokes.
func addParameters(info *types.Info, call *ast.CallExpr, add func(types.Type)) {
	t := info.TypeOf(call.Fun)
	if t == nil {
		return
	}
	sig, ok := t.Underlying().(*types.Signature)
	if !ok {
		return
	}
	params := sig.Params()
	for i := range params.Len() {
		pt := params.At(i).Type()
		if slice, ok := pt.(*types.Slice); ok && sig.Variadic() && i == params.Len()-1 {
			pt = slice.Elem()
		}
		add(pt)
	}
}

// satisfies reports whether the type, or a pointer to it, implements iface. A
// generic type is tested instantiated with its own type parameters, as
// staticcheck's unused check does: types.Implements is false for an
// uninstantiated generic type.
func satisfies(named *types.Named, iface *types.Interface) bool {
	if params := named.TypeParams(); params.Len() > 0 && named.TypeArgs().Len() == 0 {
		args := make([]types.Type, params.Len())
		for i := range args {
			args[i] = params.At(i)
		}
		inst, err := types.Instantiate(nil, named, args, false)
		if err != nil {
			return false
		}
		named = inst.(*types.Named)
	}
	return types.Implements(named, iface) || types.Implements(types.NewPointer(named), iface)
}

// overridesPromoted reports whether a struct type embeds a field that has a
// method of this name: the declaration replaces a promoted method, so the
// embedded type's contract is what it keeps.
func overridesPromoted(named *types.Named, pkg *types.Package, name string) bool {
	st, ok := named.Underlying().(*types.Struct)
	if !ok {
		return false
	}
	for field := range st.Fields() {
		if !field.Embedded() {
			continue
		}
		obj, _, _ := types.LookupFieldOrMethod(field.Type(), true, pkg, name)
		if _, isMethod := obj.(*types.Func); isMethod {
			return true
		}
	}
	return false
}

func declaredIn(t types.Type, pkg *types.Package) bool {
	named, ok := receiverNamed(t)
	return ok && named.Obj().Pkg() == pkg
}
