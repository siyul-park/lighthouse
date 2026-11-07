package provider

import "go/types"

// methodIndex lists interfaces by the names of their methods, each once.
type methodIndex struct {
	byName map[string][]*types.Interface
	seen   map[*types.Interface]bool
}

func newMethodIndex() *methodIndex {
	return &methodIndex{byName: map[string][]*types.Interface{}, seen: map[*types.Interface]bool{}}
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
