package provider

import (
	"go/types"
	"sync"

	"golang.org/x/tools/go/packages"
)

// wider are the interfaces beyond what a package names: every interface
// declared in a package of the project, and the exported interfaces of the
// packages its code can import, however indirectly. A type satisfies such an
// interface without the package ever writing its name.
type wider struct {
	once sync.Once
	// project are the interfaces declared in the project's packages.
	project []*types.Interface
	// exported are the exported interfaces of a package, by package path.
	exported sync.Map
}

// methodIndex lists interfaces by the names of their methods, each once.
type methodIndex struct {
	byName map[string][]*types.Interface
	seen   map[*types.Interface]bool
}

func (m *methodIndex) add(list ...*types.Interface) {
	for _, iface := range list {
		if m.seen[iface] {
			continue
		}
		m.seen[iface] = true
		for i := range iface.NumMethods() {
			name := iface.Method(i).Name()
			m.byName[name] = append(m.byName[name], iface)
		}
	}
}

// widerInterfaces returns the interfaces a package may satisfy without naming
// them, by method name.
func (r *resolver) widerInterfaces(pkg *packages.Package) map[string][]*types.Interface {
	r.wider.once.Do(func() {
		for _, p := range r.declared {
			r.wider.project = append(r.wider.project, interfacesIn(p.Types, false)...)
		}
	})
	found := newMethodIndex()
	found.add(r.wider.project...)
	packages.Visit([]*packages.Package{pkg}, nil, func(p *packages.Package) {
		cached, ok := r.wider.exported.Load(p.PkgPath)
		if !ok {
			cached, _ = r.wider.exported.LoadOrStore(p.PkgPath, interfacesIn(p.Types, true))
		}
		found.add(cached.([]*types.Interface)...)
	})
	return found.byName
}

func newMethodIndex() *methodIndex {
	return &methodIndex{byName: map[string][]*types.Interface{}, seen: map[*types.Interface]bool{}}
}

// interfacesIn lists the interfaces declared at the top level of a package:
// those with methods, not generic. Only the exported ones when asked.
func interfacesIn(pkg *types.Package, exportedOnly bool) []*types.Interface {
	if pkg == nil {
		return nil
	}
	var list []*types.Interface
	scope := pkg.Scope()
	for _, name := range scope.Names() {
		tn, ok := scope.Lookup(name).(*types.TypeName)
		if !ok || (exportedOnly && !tn.Exported()) || tn.IsAlias() {
			continue
		}
		if iface, ok := candidate(tn.Type()); ok {
			list = append(list, iface)
		}
	}
	return list
}

// candidate returns the interface a type is, when it is one worth matching:
// not generic, with methods, a method set rather than a constraint.
func candidate(t types.Type) (*types.Interface, bool) {
	if named, ok := types.Unalias(t).(*types.Named); ok && named.TypeParams().Len() > 0 {
		return nil, false
	}
	iface, ok := t.Underlying().(*types.Interface)
	return iface, ok && iface.NumMethods() > 0 && iface.IsMethodSet()
}
