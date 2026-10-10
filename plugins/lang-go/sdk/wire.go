// Package sdk speaks the Lighthouse plugin protocol: LSP-framed JSON-RPC 2.0
// and the wire types of docs/plugin-protocol.md. It knows nothing about
// languages or Lighthouse rules.
package sdk

import "encoding/json"

// ClientInfo identifies the host in InitializeParams.
type ClientInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// InitializeParams is the request of the `initialize` method.
type InitializeParams struct {
	Root            string     `json:"root"`
	ProtocolVersion string     `json:"protocolVersion"`
	ClientInfo      ClientInfo `json:"clientInfo"`
}

// InitializeResult is the response of the `initialize` method.
type InitializeResult struct {
	ID              string             `json:"id"`
	Version         string             `json:"version"`
	ProtocolVersion string             `json:"protocolVersion"`
	Languages       []ProviderManifest `json:"languages"`
}

// ProviderManifest is what a language provider declares: the language a
// plugin serves, how files select it and what it guarantees.
type ProviderManifest struct {
	ID           string      `json:"id"`
	Globs        []string    `json:"globs"`
	Priority     int         `json:"priority"`
	Fallback     bool        `json:"fallback,omitempty"`
	Conventions  Conventions `json:"conventions"`
	Capabilities []string    `json:"capabilities"`
}

// Conventions are the file conventions a language declares for the whole host.
type Conventions struct {
	TestGlobs []string `json:"test_globs"`
	// ConstructorPrefixes are the name prefixes that make a function a constructor.
	ConstructorPrefixes []string `json:"constructor_prefixes"`
}

// ProjectRef locates the project an index request refers to.
type ProjectRef struct {
	Root string `json:"root"`
}

// FileRef names a project-relative file and its content hash.
type FileRef struct {
	Path string `json:"path"`
	Hash string `json:"hash"`
}

// Overlay is reserved; hosts do not send overlays in protocol 0.1.
type Overlay struct {
	Path string `json:"path"`
	Text string `json:"text"`
}

// CacheRef is the directory a provider may keep derived results in.
type CacheRef struct {
	Dir string `json:"dir"`
}

// Context carries the per-language options of lighthouse.toml.
type Context struct {
	Options  map[string]json.RawMessage `json:"options"`
	Overlays []Overlay                  `json:"overlays,omitempty"`
	// Cache is absent when the host runs without a cache.
	Cache *CacheRef `json:"cache,omitempty"`
}

// IndexParams is the request of the `index` method.
type IndexParams struct {
	Project  ProjectRef `json:"project"`
	Language string     `json:"language"`
	Files    []FileRef  `json:"files"`
	Context  Context    `json:"context"`
}

// IndexResult is the response of the `index` method. Its lists must encode as
// `[]` when empty, never `null`.
type IndexResult struct {
	Fragments  []Fragment   `json:"fragments"`
	Notices    []string     `json:"notices"`
	Incomplete []Incomplete `json:"incomplete"`
	// Encoded are fragments that are already JSON, merged in by path when the
	// result is encoded; Fragments and Encoded never hold the same path.
	Encoded []EncodedFragment `json:"-"`
}

// Incomplete reports why a file, or the whole request when Path is empty, was
// not fully analyzed.
type Incomplete struct {
	Path   string `json:"path,omitempty"`
	Reason string `json:"reason"`
}

// Fragment is the code model of one file.
type Fragment struct {
	File      FileInfo          `json:"file"`
	Modules   []Module          `json:"modules"`
	Symbols   []Symbol          `json:"symbols"`
	Edges     []Edge            `json:"edges"`
	Functions []FunctionSummary `json:"functions"`
	Tests     []TestCase        `json:"tests"`
	Comments  []Comment         `json:"comments"`
}

// FileInfo identifies the file of a Fragment.
type FileInfo struct {
	Path      string `json:"path"`
	Generated bool   `json:"generated,omitempty"`
}

// Module is a unit of dependency such as a package or directory.
type Module struct {
	Path   string `json:"path"`
	Name   string `json:"name,omitempty"`
	TestOf string `json:"test_of,omitempty"`
}

// Position is a one-based line and column.
type Position struct {
	Line int `json:"line"`
	Col  int `json:"col"`
}

// Span is a source range.
type Span struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

// Symbol is a declared name with its kind, owner and location.
type Symbol struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"`
	Visibility string `json:"visibility"`
	Owner      string `json:"owner,omitempty"`
	File       string `json:"file"`
	Span       Span   `json:"span"`
	// Extent is the whole declaration, doc comments included; nil when the
	// provider reports none for the symbol.
	Extent *Span  `json:"extent,omitempty"`
	Doc    string `json:"doc,omitempty"`
	Name   string `json:"name"`
	Role   string `json:"role,omitempty"`
	// Optional marks a field a caller may leave out: a pointer, slice, map,
	// function, channel or interface.
	Optional bool `json:"optional,omitempty"`
	// TypeRef is the type of a field.
	TypeRef *TypeRef `json:"type_ref,omitempty"`
}

// TypeRef is a type as a signature or a field writes it: Text is the type with
// full package paths, Symbol the project type it names (a kind-less symbol
// id), Exported whether that type is exported.
type TypeRef struct {
	Text     string `json:"text"`
	Symbol   string `json:"symbol,omitempty"`
	Exported *bool  `json:"exported,omitempty"`
}

// Signature is the parameter and result types of a function, receiver
// excluded, one entry per occurrence.
type Signature struct {
	Params  []TypeRef `json:"params"`
	Results []TypeRef `json:"results"`
}

// Event is something a function body does that a rule may flag.
type Event struct {
	Kind   string `json:"kind"`
	Span   Span   `json:"span"`
	Detail string `json:"detail,omitempty"`
}

// Node is a module or a symbol: exactly one field is set.
type Node struct {
	Module string `json:"module,omitempty"`
	Symbol string `json:"symbol,omitempty"`
}

// Edge is a relation from a Node to a target symbol or module.
type Edge struct {
	Kind       string `json:"kind"`
	From       Node   `json:"from"`
	To         string `json:"to"`
	Resolution string `json:"resolution"`
	// Site is the span of the identifier that names the target.
	Site *Span `json:"site,omitempty"`
}

// Flow is one normalized control-flow event of a function body.
type Flow struct {
	Kind      string `json:"kind"`
	Nesting   int    `json:"nesting"`
	Arms      int    `json:"arms,omitempty"`
	Operators int    `json:"operators,omitempty"`
	Returning bool   `json:"returning,omitempty"`
}

// FunctionSummary is the measured shape of one function or method.
type FunctionSummary struct {
	Symbol           string     `json:"symbol"`
	MaxNesting       int        `json:"max_nesting"`
	Statements       int        `json:"statements"`
	TopLevel         int        `json:"top_level"`
	Params           int        `json:"params"`
	Returns          int        `json:"returns"`
	Tokens           int        `json:"tokens"`
	Flow             []Flow     `json:"flow"`
	CloneFingerprint string     `json:"clone_fingerprint,omitempty"`
	ForwardsTo       string     `json:"forwards_to,omitempty"`
	Signature        *Signature `json:"signature,omitempty"`
	Events           []Event    `json:"events,omitempty"`
	// Implementation marks a method that satisfies an interface; Constructs a
	// function without a receiver that returns a type of its package.
	Implementation   bool `json:"implementation,omitempty"`
	Constructs       bool `json:"constructs,omitempty"`
	ManualAssertions int  `json:"manual_assertions,omitempty"`
}

// TestCase describes how a test entry point is written and what it targets.
type TestCase struct {
	Symbol  string   `json:"symbol"`
	Nesting int      `json:"nesting"`
	Style   string   `json:"style"`
	Targets []string `json:"targets"`
}

// Comment is a comment group of a file: adjacent comments with no blank line
// between them, as the source reads, markers included.
type Comment struct {
	Span       Span   `json:"span"`
	Text       string `json:"text"`
	AttachedTo string `json:"attached_to,omitempty"`
}

// ProtocolVersion is the wire protocol version this package implements.
const ProtocolVersion = "0.1"

// SemanticEdges names the capability of reporting type-resolved edges.
const SemanticEdges = "semantic-edges"

// Extent names the capability of reporting the full range of declarations.
const Extent = "extent"

// Overlays names the capability of analyzing overlay text instead of the disk.
const Overlays = "overlays"

// ReferenceSites names the capability of reporting where references occur.
const ReferenceSites = "reference-sites"
