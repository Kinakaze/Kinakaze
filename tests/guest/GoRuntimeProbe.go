package main

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
)

func main() {
	// Go uses raw clone/vfork, dup3 and execve while other runtime threads
	// create and retire pipes. Repeat to expose descriptor snapshot races.
	var workers sync.WaitGroup
	failures := make(chan error, 8)
	for worker := 0; worker < cap(failures); worker++ {
		workers.Add(1)
		go func() {
			defer workers.Done()
			for round := 0; round < 3; round++ {
				command := exec.Command("/bin/sh", "-c", "cat; printf '%s' \"$PROBE_VALUE\" >&2")
				command.Env = append(os.Environ(), "PROBE_VALUE=stderr-value")
				command.Stdin = strings.NewReader("stdin-value\n")
				var stdout, stderr bytes.Buffer
				command.Stdout, command.Stderr = &stdout, &stderr
				if err := command.Run(); err != nil {
					failures <- err
					return
				}
				if stdout.String() != "stdin-value\n" || stderr.String() != "stderr-value" {
					failures <- fmt.Errorf("redirected streams: stdout=%q stderr=%q", stdout.String(), stderr.String())
					return
				}
			}
		}()
	}
	workers.Wait()
	close(failures)
	for err := range failures {
		panic(err)
	}
	var exited *exec.ExitError
	if err := exec.Command("/bin/sh", "-c", "exit 42").Run(); !errors.As(err, &exited) || exited.ExitCode() != 42 {
		panic(fmt.Sprintf("child exit status: %v", err))
	}
	if err := exec.Command("/nonexistent-go-runtime-probe").Run(); !os.IsNotExist(err) {
		panic(fmt.Sprintf("failed exec: %v", err))
	}
	fmt.Println("GO_OK")
}
