package provider

import (
	"go/types"
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

type declaredType struct {
	obj  *types.TypeName
	unit *unit
}

// implements adds an `implements` edge from every declared type to every
// declared interface it satisfies, by value or by pointer. Interfaces of one
// method are skipped unless asked for: nearly every type satisfies them by
// accident.
func (r *run) implements(units []*unit, res *resolver) {
	byFile := map[string]*unit{}
	for _, u := range units {
		byFile[u.rel] = u
	}
	var ifaces, concrete []declaredType
	seen := map[*types.Package]bool{}
	for _, u := range units {
		if seen[u.pkg.Types] {
			continue
		}
		seen[u.pkg.Types] = true
		scope := u.pkg.Types.Scope()
		for _, name := range scope.Names() {
			obj, ok := scope.Lookup(name).(*types.TypeName)
			if !ok || obj.IsAlias() {
				continue
			}
			rel, ok := r.rel(u.fset.Position(obj.Pos()).Filename)
			owner := byFile[rel]
			named, isNamed := obj.Type().(*types.Named)
			if !ok || owner == nil || !isNamed || named.TypeParams().Len() > 0 {
				continue
			}
			if iface, ok := named.Underlying().(*types.Interface); ok {
				if iface.NumMethods() > 1 || (iface.NumMethods() == 1 && r.opts.SmallInterfaces) {
					ifaces = append(ifaces, declaredType{obj, owner})
				}
				continue
			}
			concrete = append(concrete, declaredType{obj, owner})
		}
	}
	sort.Slice(concrete, func(i, j int) bool { return concrete[i].obj.Pos() < concrete[j].obj.Pos() })
	for _, t := range concrete {
		for _, i := range ifaces {
			iface := i.obj.Type().Underlying().(*types.Interface)
			if !types.Implements(t.obj.Type(), iface) && !types.Implements(types.NewPointer(t.obj.Type()), iface) {
				continue
			}
			from := symbolID(kindType, t.unit.module, t.obj.Name())
			edge := sdk.Edge{
				Kind:       edgeImplements,
				From:       sdk.Node{Symbol: from},
				To:         target(res.module(i.obj.Pkg()), i.obj.Name()),
				Resolution: resolutionSemantic,
			}
			frag := r.fragments[t.unit.rel]
			frag.Edges = append(frag.Edges, edge)
		}
	}
}
