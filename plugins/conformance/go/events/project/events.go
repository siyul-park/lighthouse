package events

import (
	"context"
	"errors"
	"fmt"
	"io"
)

var ErrNotFound = errors.New("not found")

type secret struct{}

type Custom struct{}

func (*Custom) Error() string { return "custom" }

// Is compares errors on purpose: it is the method errors.Is calls.
func (*Custom) Is(target error) bool { return target == ErrNotFound }

type Reader struct {
	ctx  context.Context
	name string
	cfg  *secret
}

func Fetch(name string, ctx context.Context) error { return ctx.Err() }

func Open(ctx context.Context, name string) (*secret, error) { return nil, nil }

func Must(v int) int {
	if v < 0 {
		panic("negative")
	}
	return v
}

func Compare(err error) bool { return err == ErrNotFound }

func NotNil(err error) bool { return err != nil }

func AtEnd(err error) bool { return err == io.EOF }

func Wrapped(err error) error { return fmt.Errorf("fetch: %v", err) }

func Wrapping(err error) error { return fmt.Errorf("fetch: %w", err) }

func Plain(name string) error { return fmt.Errorf("fetch %s", name) }

func Assert(err error) string {
	if e, ok := err.(*Custom); ok {
		return e.Error()
	}
	return ""
}

func Kind(err error) int {
	switch err.(type) {
	case *Custom:
		return 1
	}
	return 0
}

func Choose(err error) int {
	switch err {
	case ErrNotFound:
		return 1
	}
	return 0
}

func Closure(err error) func() bool {
	return func() bool { return err == ErrNotFound }
}
