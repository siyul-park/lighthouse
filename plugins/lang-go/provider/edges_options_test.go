package provider_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

func TestConfiguresNamesTheTypeTheOptionsBuild(t *testing.T) {
	tests := []struct {
		name  string
		files map[string]string
		want  map[string]string
	}{
		{
			name: "a private option struct is built into the type of its constructor",
			files: map[string]string{"p.go": `package p

type Server struct{}
type option struct{ fuel int }
type Option func(*option)

func New(opts ...Option) *Server { return &Server{} }
func WithFuel(n int) Option      { return func(o *option) { o.fuel = n } }
`},
			want: map[string]string{"WithFuel": "Server"},
		},
		{
			name: "a constructor returning an error builds its type",
			files: map[string]string{"p.go": `package p

type Server struct{}
type option struct{}

func New(opts ...func(*option)) (Server, error) { return Server{}, nil }
func WithFuel() func(*option)                   { return func(*option) {} }
`},
			want: map[string]string{"WithFuel": "Server"},
		},
		{
			name: "of two constructors the one beside the builder wins",
			files: map[string]string{
				"a.go": `package p

type A struct{}
type option struct{}
type Option func(*option)

func NewA(opts ...Option) *A { return &A{} }
func WithA() Option          { return func(*option) {} }
`,
				"b.go": `package p

type B struct{}

func NewB(opts ...Option) *B { return &B{} }
func WithB() Option          { return func(*option) {} }
`,
			},
			want: map[string]string{"WithA": "A", "WithB": "B"},
		},
		{
			name: "without a constructor the builder configures the parameter type",
			files: map[string]string{"p.go": `package p

type option struct{}
type Option func(*option)

func WithFuel() Option { return func(*option) {} }
`},
			want: map[string]string{"WithFuel": "option"},
		},
		{
			name: "several constructors beside the builder leave the parameter type",
			files: map[string]string{"p.go": `package p

type A struct{}
type B struct{}
type option struct{}
type Option func(*option)

func NewA(opts ...Option) *A { return &A{} }
func NewB(opts ...Option) *B { return &B{} }
func WithFuel() Option       { return func(*option) {} }
`},
			want: map[string]string{"WithFuel": "option"},
		},
		{
			name: "an option func of the type itself configures it",
			files: map[string]string{"p.go": `package p

type Server struct{}

func WithFuel() func(*Server) { return func(*Server) {} }
`},
			want: map[string]string{"WithFuel": "Server"},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			require.Equal(t, tc.want, configured(t, tc.files))
		})
	}
}

func configured(t *testing.T, files map[string]string) map[string]string {
	t.Helper()
	root := t.TempDir()
	require.NoError(t, os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/p\n\ngo 1.22\n"), 0o600))
	var refs []sdk.FileRef
	for name, body := range files {
		require.NoError(t, os.WriteFile(filepath.Join(root, name), []byte(body), 0o600))
		refs = append(refs, sdk.FileRef{Path: name})
	}
	result, err := indexOf(provider.New("lang-go", "test"), sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Units:    oneUnit(refs),
		Files:    filesOf(refs),
	})
	require.NoError(t, err)
	require.Empty(t, result.Incomplete)
	names := map[string]string{}
	for _, fragment := range result.Fragments {
		for _, symbol := range fragment.Symbols {
			names[symbol.ID] = symbol.Name
		}
	}
	got := map[string]string{}
	for _, fragment := range result.Fragments {
		for _, fn := range fragment.Functions {
			if fn.Configures != "" {
				_, owner, _ := strings.Cut(fn.Configures, "::")
				got[names[fn.Symbol]] = owner
			}
		}
	}
	return got
}
