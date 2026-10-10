package sample

import "fmt"

func Load(name string, err error) error {
    return fmt.Errorf("load %s: %v", name, err)
}
