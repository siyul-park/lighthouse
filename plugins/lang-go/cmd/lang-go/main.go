// Command lang-go is the Lighthouse language plugin for Go. It speaks the
// plugin protocol on stdin and stdout.
package main

import (
	"fmt"
	"os"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

const (
	id      = "lang-go"
	version = "0.1.0"
)

func main() {
	if err := sdk.Serve(os.Stdin, os.Stdout, provider.New(id, version)); err != nil {
		fmt.Fprintln(os.Stderr, "lang-go:", err)
		os.Exit(1)
	}
}
