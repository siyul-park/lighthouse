package options

// Program is configured with options.
type Program struct{ name string }

// Option configures a Program.
type Option func(*Program)

// WithName names the program.
func WithName(name string) func(*Program) {
	return func(p *Program) { p.name = name }
}

// WithAlias is an option of the named type.
func WithAlias(name string) Option {
	return func(p *Program) { p.name = name }
}

// Apply is not an option: it takes two arguments.
func Apply(p *Program, o Option) func(*Program, *Program) {
	o(p)
	return nil
}
