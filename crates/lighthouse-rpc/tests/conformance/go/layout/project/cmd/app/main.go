package main

import (
	"fmt"

	helpers "example.com/layout/internal/text"
)

// Version is exported but nothing can import a main package.
var Version = "1"

// Greeting is built at start.
func Greeting() string { return fmt.Sprint(helpers.Count("hello")) }

func main() { fmt.Println(Greeting()) }
