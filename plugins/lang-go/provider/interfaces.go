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

// widerInterfaces returns the interfaces a package may satisfy without naming
// them, by method name.
func (r *resolver) widerInterfaces(pkg *packages.Package) map[string][]*types.Interface {
	r.wider.once.Do(func() {
		for _, p := range r.declared {
			r.wider.project = append(r.wider.project, interfacesIn(p.Types, false)...)
		}
	})
	found := map[string][]*types.Interface{}
	seen := map[*types.Interface]bool{}
	add := func(list []*types.Interface) {
		for _, iface := range list {
			if seen[iface] {
				continue
			}
			seen[iface] = true
			for i := range iface.NumMethods() {
				name := iface.Method(i).Name()
				found[name] = append(found[name], iface)
			}
		}
	}
	add(r.wider.project)
	packages.Visit([]*packages.Package{pkg}, nil, func(p *packages.Package) {
		cached, ok := r.wider.exported.Load(p.PkgPath)
		if !ok {
			cached, _ = r.wider.exported.LoadOrStore(p.PkgPath, interfacesIn(p.Types, true))
		}
		add(cached.([]*types.Interface))
	})
	return found
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
		if named, ok := tn.Type().(*types.Named); ok && named.TypeParams().Len() > 0 {
			continue
		}
		iface, ok := tn.Type().Underlying().(*types.Interface)
		if ok && iface.NumMethods() > 0 && iface.IsMethodSet() {
			list = append(list, iface)
		}
	}
	return list
}
