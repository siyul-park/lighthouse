package provider

import (
	"go/ast"
	"go/types"
	"sync"

	"golang.org/x/tools/go/packages"
)

// interfaceSet is the interfaces the code of a package names, by method name.
type interfaceSet struct {
	once     sync.Once
	byMethod map[string][]*types.Interface
	// embedders are the types of the package that embed a type, by the
	// embedded type: its methods are promoted into them.
	embedders map[*types.TypeName][]*types.Named
}

// roles: what a function is for, as far as the types say. A method whose
// signature an interface dictates is an implementation; a function without a
// receiver that returns a type of its package builds it.

// implementation reports whether the method satisfies an interface its
// package names: some candidate interface has a method of the same name and
// the receiver type (or a pointer to it) implements the interface.
func (u *unit) implementation(d *ast.FuncDecl) bool {
	if d.Recv == nil {
		return false
	}
	fn, ok := u.info().Defs[d.Name].(*types.Func)
	if !ok {
		return false
	}
	recv := fn.Type().(*types.Signature).Recv()
	if recv == nil {
		return false
	}
	named, ok := receiverNamed(recv.Type())
	if !ok || named.TypeParams().Len() > 0 {
		return false
	}
	if overridesPromoted(named, u.pkg.Types, d.Name.Name) {
		return true
	}
	set := u.res.interfacesOf(u.pkg)
	for _, iface := range set.byMethod[d.Name.Name] {
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

func declaredIn(t types.Type, pkg *types.Package) bool {
	named, ok := receiverNamed(t)
	return ok && named.Obj().Pkg() == pkg
}

func isError(t types.Type) bool { return t != nil && types.Identical(t, errorType) }

// receiverNamed is the named type behind an optional pointer.
func receiverNamed(t types.Type) (*types.Named, bool) {
	t = types.Unalias(t)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	named, ok := t.(*types.Named)
	return named, ok
}

// interfacesOf returns the candidate interfaces of a package, computed once:
// those it names or declares (types.Info.Uses and Defs) and those its calls take as parameters, so
// an interface of a dependency is found when the package hands a value to it.
func (r *resolver) interfacesOf(pkg *packages.Package) *interfaceSet {
	cached, _ := r.interfaces.LoadOrStore(pkg, &interfaceSet{})
	set := cached.(*interfaceSet)
	set.once.Do(func() {
		set.byMethod = collectInterfaces(pkg)
		set.embedders = collectEmbedders(pkg)
	})
	return set
}

func collectInterfaces(pkg *packages.Package) map[string][]*types.Interface {
	found := map[string][]*types.Interface{}
	seen := map[*types.Interface]bool{}
	add := func(t types.Type) {
		if named, ok := types.Unalias(t).(*types.Named); ok && named.TypeParams().Len() > 0 {
			return
		}
		iface, ok := t.Underlying().(*types.Interface)
		if !ok || seen[iface] || iface.NumMethods() == 0 || !iface.IsMethodSet() {
			return
		}
		seen[iface] = true
		for i := range iface.NumMethods() {
			name := iface.Method(i).Name()
			found[name] = append(found[name], iface)
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
	return found
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

// satisfies reports whether the type, or a pointer to it, implements iface.
func satisfies(named *types.Named, iface *types.Interface) bool {
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
