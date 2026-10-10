package provider

import (
	"go/ast"
	"go/types"
	"sync"

	"golang.org/x/tools/go/packages"
)

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
	for _, iface := range u.res.interfacesOf(u.pkg).byMethod[d.Name.Name] {
		if types.Implements(named, iface) || types.Implements(types.NewPointer(named), iface) {
			return true
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

func isError(t types.Type) bool { return types.Identical(t, types.Universe.Lookup("error").Type()) }

// receiverNamed is the named type behind an optional pointer.
func receiverNamed(t types.Type) (*types.Named, bool) {
	t = types.Unalias(t)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	named, ok := t.(*types.Named)
	return named, ok
}

// interfaceSet is the interfaces the code of a package names, by method name.
type interfaceSet struct {
	once     sync.Once
	byMethod map[string][]*types.Interface
}

// interfacesOf returns the candidate interfaces of a package, computed once:
// those it names or declares (types.Info.Uses and Defs) and those its calls take as parameters, so
// an interface of a dependency is found when the package hands a value to it.
func (r *resolver) interfacesOf(pkg *packages.Package) *interfaceSet {
	cached, _ := r.interfaces.LoadOrStore(pkg, &interfaceSet{})
	set := cached.(*interfaceSet)
	set.once.Do(func() { set.byMethod = collectInterfaces(pkg) })
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
