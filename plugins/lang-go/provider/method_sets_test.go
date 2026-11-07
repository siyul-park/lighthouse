package provider_test

import (
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

// methodFacts are the method-set facts of the symbols and methods of a module,
// by symbol id.
type methodFacts struct {
	provides, requires map[string][]string
	edges              map[string]bool
	pending            map[string]sdk.Pending
	implementation     map[string]bool
}

func TestProviderMethodKeys(t *testing.T) {
	module := func(files map[string]string) methodFacts {
		root := t.TempDir()
		files["go.mod"] = testModule
		writeTree(t, root, files)
		return factsOf(t, root)
	}

	t.Run("an alias is the type it stands for", func(t *testing.T) {
		got := module(map[string]string{"p/p.go": "package p\n\ntype Name = string\n\ntype I interface{ Get(n Name, b byte) (Name, error) }\n\n" +
			"type T struct{}\n\nfunc (T) Get(n string, b uint8) (string, error) { return n, nil }\n"})

		require.NotEmpty(t, got.requires["p::I#interface"])
		require.Equal(t, got.requires["p::I#interface"], got.provides["p::T#type"])
	})

	t.Run("a type is told from another of the same name by its package path", func(t *testing.T) {
		got := module(map[string]string{
			"a/x/x.go": "package x\n\ntype V int\n",
			"b/x/x.go": "package x\n\ntype V int\n",
			"p/p.go": "package p\n\nimport (\n\ta \"example.com/m/a/x\"\n\tb \"example.com/m/b/x\"\n)\n\n" +
				"type A struct{}\n\nfunc (A) M(v a.V) {}\n\ntype B struct{}\n\nfunc (B) M(v b.V) {}\n",
		})

		require.NotEqual(t, got.provides["p::A#type"], got.provides["p::B#type"])
	})

	t.Run("an unexported method is told from another of the same name by its package", func(t *testing.T) {
		got := module(map[string]string{
			"a/a.go": "package a\n\ntype T struct{}\n\nfunc (T) m() {}\n",
			"b/b.go": "package b\n\ntype T struct{}\n\nfunc (T) m() {}\n",
		})

		require.NotEqual(t, got.provides["a::T#type"], got.provides["b::T#type"])
	})

	t.Run("an interface requires the methods of the interfaces it embeds", func(t *testing.T) {
		got := module(map[string]string{"p/p.go": "package p\n\ntype I interface{ A() }\n\ntype J interface {\n\tI\n\tB()\n}\n\n" +
			"type T struct{}\n\nfunc (T) A() {}\n\nfunc (T) B() {}\n"})

		require.Equal(t, []string{"A:func()()", "B:func()()"}, got.requires["p::J#interface"])
		require.Equal(t, got.requires["p::J#interface"], got.provides["p::T#type"])
		require.True(t, got.edges["p::J#interface"])
		require.False(t, got.edges["p::I#interface"], "an interface of one method has no edges")
	})

	t.Run("a generic type provides the methods of its own instance", func(t *testing.T) {
		got := module(map[string]string{"p/p.go": "package p\n\ntype G[T any] struct{ v T }\n\nfunc (g *G[T]) Len() int { return 0 }\n\nfunc (g G[T]) Get() T { return g.v }\n"})

		require.Len(t, got.provides["p::G#type"], 2)
		require.Contains(t, got.provides["p::G#type"], "Len:func()(int)")
	})

	t.Run("a promoted method is provided", func(t *testing.T) {
		got := module(map[string]string{"p/p.go": "package p\n\ntype Base struct{}\n\nfunc (*Base) A() {}\n\ntype Outer struct{ Base }\n"})

		require.Equal(t, []string{"A:func()()"}, got.provides["p::Outer#type"])
	})

	t.Run("a struct tag is part of the type", func(t *testing.T) {
		got := module(map[string]string{"p/p.go": "package p\n\ntype A struct{}\n\nfunc (A) M(s struct{ N int `json:\"n\"` }) {}\n\n" +
			"type B struct{}\n\nfunc (B) M(s struct{ N int `json:\"n\"` }) {}\n\n" +
			"type C struct{}\n\nfunc (C) M(s struct{ N int `json:\"m\"` }) {}\n"})

		require.Equal(t, got.provides["p::A#type"], got.provides["p::B#type"])
		require.NotEqual(t, got.provides["p::A#type"], got.provides["p::C#type"])
	})
}

func TestProviderPendingMethods(t *testing.T) {
	root := t.TempDir()
	writeTree(t, root, map[string]string{
		"go.mod": testModule,
		"p/p.go": "package p\n\ntype Base struct{}\n\nfunc (Base) Run() {}\n\ntype Job struct{ Base }\n\nfunc (Job) Own() {}\n",
		"q/q.go": "package q\n\ntype Runner interface {\n\tRun()\n\tStop()\n}\n",
	})

	got := factsOf(t, root, "p")

	t.Run("a method no interface in view settles is pending, with the types that carry it", func(t *testing.T) {
		require.False(t, got.implementation["p::Base::Run#method"])
		require.Equal(t, sdk.Pending{Key: "Run:func()()", Carriers: []string{"p::Base#type", "p::Job#type"}}, got.pending["p::Base::Run#method"])
	})

	t.Run("the interface of a package that is not loaded is not read", func(t *testing.T) {
		require.NotContains(t, got.requires, "q::Runner#interface")
	})
}

func factsOf(t *testing.T, root string, ids ...string) methodFacts {
	t.Helper()
	facts := methodFacts{
		provides: map[string][]string{}, requires: map[string][]string{}, edges: map[string]bool{},
		pending: map[string]sdk.Pending{}, implementation: map[string]bool{},
	}
	units := unitRefs(t, root)
	if len(ids) > 0 {
		var asked []sdk.UnitRef
		for _, unit := range units {
			for _, id := range ids {
				if unit.ID == id {
					asked = append(asked, unit)
				}
			}
		}
		units = asked
	}
	result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
		Project: sdk.ProjectRef{Root: root}, Language: "go", Units: units, Files: projectFiles(t, root),
	})
	require.NoError(t, err)
	for _, unit := range result.Units {
		for _, fragment := range unit.Fragments {
			for _, s := range fragment.Symbols {
				facts.provides[s.ID], facts.requires[s.ID], facts.edges[s.ID] = s.Provides, s.Requires, s.Edges
			}
			for _, f := range fragment.Functions {
				if f.Pending != nil {
					facts.pending[f.Symbol] = *f.Pending
				}
				facts.implementation[f.Symbol] = f.Implementation
			}
		}
	}
	return facts
}
