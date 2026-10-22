// Reference agent loop in Go (CHECKLIST 2.020, PROPOSAL section 4.1).
//
// Same contract as the Python/TypeScript loops: check (diagnose) via
// eval envelope diagnostics -> resilient eval (repair by the engine) ->
// re-emit via the `active import` CLI -> check again (validate) ->
// final redacted JSON. Budgeted iterations, stuck detection, code-only
// secret-safe logs. The WASM module has no imports, so plain wazero
// instantiation suffices; Allocator Model A holds (free everything).
//
// Usage from the repo root:
//
//	go run ./agent-kit/reference-loops/go <candidate.active> [--max-iters=N]
package main

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
)

var codeRe = regexp.MustCompile(`\b([A-Z][0-9]{3})\b`)

type handle struct {
	ctx    context.Context
	cancel context.CancelFunc
	rt     wazero.Runtime
	mod    api.Module
	mem    api.Memory
	alloc  api.Function
	eval   api.Function
	free   api.Function
}

func openModule(wasmPath string) (*handle, error) {
	ctx, cancel := context.WithCancel(context.Background())
	rt := wazero.NewRuntime(ctx)
	wasm, err := os.ReadFile(wasmPath)
	if err != nil {
		cancel()
		rt.Close(context.Background())
		return nil, err
	}
	compiled, err := rt.CompileModule(ctx, wasm)
	if err != nil {
		cancel()
		rt.Close(context.Background())
		return nil, fmt.Errorf("compile: %w", err)
	}
	defer compiled.Close(ctx)
	inst, err := rt.InstantiateModule(ctx, compiled, wazero.NewModuleConfig())
	if err != nil {
		cancel()
		rt.Close(context.Background())
		return nil, fmt.Errorf("instantiate: %w", err)
	}
	h := &handle{ctx: ctx, cancel: cancel, rt: rt, mod: inst}
	h.mem = inst.Memory()
	h.alloc = inst.ExportedFunction("active_alloc")
	h.eval = inst.ExportedFunction("active_eval_json")
	h.free = inst.ExportedFunction("active_free")
	abi := inst.ExportedFunction("active_abi_version")
	out, err := abi.Call(ctx)
	if err != nil || len(out) == 0 || out[0] != 1 {
		h.close()
		return nil, fmt.Errorf("ABI major mismatch")
	}
	return h, nil
}

func (h *handle) close() {
	h.cancel()
	h.rt.Close(context.Background())
}

func (h *handle) write(s string) (uint32, uint64, error) {
	b := []byte(s)
	out, err := h.alloc.Call(h.ctx, uint64(len(b)))
	if err != nil || len(out) == 0 {
		return 0, 0, fmt.Errorf("alloc: %v", err)
	}
	ptr := uint32(out[0])
	if !h.mem.Write(ptr, b) {
		return 0, 0, fmt.Errorf("write OOB")
	}
	return ptr, uint64(len(b)), nil
}

func (h *handle) u32(ptr uint32) (uint32, bool) {
	b, ok := h.mem.Read(ptr, 4)
	if !ok {
		return 0, false
	}
	return binary.LittleEndian.Uint32(b), true
}

type envelope struct {
	OK          bool            `json:"ok"`
	JSON        json.RawMessage `json:"json"`
	Diagnostics string          `json:"diagnostics"`
}

func (h *handle) evaluate(source string) (*envelope, error) {
	sp, sn, err := h.write(source)
	if err != nil {
		return nil, err
	}
	defer h.free.Call(h.ctx, uint64(sp), sn)
	slot, _, err := h.write("\x00\x00\x00\x00")
	if err != nil {
		return nil, err
	}
	defer h.free.Call(h.ctx, uint64(slot), 4)
	out, err := h.eval.Call(h.ctx, uint64(sp), sn, uint64(slot))
	if err != nil || len(out) == 0 {
		return nil, fmt.Errorf("eval trap: %v", err)
	}
	n, ok := h.u32(slot)
	if !ok {
		return nil, fmt.Errorf("read slot")
	}
	raw, ok := h.mem.Read(uint32(out[0]), n)
	if !ok {
		h.free.Call(h.ctx, out[0], uint64(n))
		return nil, fmt.Errorf("read envelope")
	}
	// Copy out of the module-owned view BEFORE freeing: Read aliases
	// linear memory, and active_free may reuse/scribble the block
	// (observed: freed bytes surface as 'X', breaking the parse of
	// T003-sized envelopes while others passed by luck). Freeing here
	// still covers every path, so a corrupt envelope cannot leak.
	buf := append([]byte(nil), raw...)
	h.free.Call(h.ctx, out[0], uint64(n))
	var env envelope
	if err := json.Unmarshal(buf, &env); err != nil {
		return nil, err
	}
	return &env, nil
}

func codesOf(diag string) []string {
	out := []string{}
	seen := map[string]bool{}
	for _, m := range codeRe.FindAllString(diag, -1) {
		if !seen[m] {
			seen[m] = true
			out = append(out, m)
		}
	}
	sort.Strings(out)
	return out
}

func repoRoot() string {
	// loop.go lives at agent-kit/reference-loops/go/ in the repo.
	abs, _ := filepath.Abs(".")
	for {
		if _, err := os.Stat(filepath.Join(abs, "Cargo.toml")); err == nil {
			return abs
		}
		parent := filepath.Dir(abs)
		if parent == abs {
			return "."
		}
		abs = parent
	}
}

func reemit(value json.RawMessage) (string, error) {
	tmp, err := os.CreateTemp("", "activeloop-*.json")
	if err != nil {
		return "", err
	}
	name := tmp.Name()
	tmp.Write(value)
	tmp.Close()
	defer os.Remove(name)
	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "cargo", "run", "-q", "-p", "active-cli",
		"--", "import", name, "--from", "json")
	cmd.Dir = repoRoot()
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("import: %v", err)
	}
	return string(out), nil
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: loop <candidate.active> [--max-iters=N]")
		os.Exit(2)
	}
	maxIters := 5
	for _, a := range os.Args[2:] {
		if strings.HasPrefix(a, "--max-iters=") {
			n, err := strconv.Atoi(strings.TrimPrefix(a, "--max-iters="))
			if err != nil {
				n = 0
			}
			maxIters = n
		}
	}
	if maxIters < 1 {
		fmt.Fprintln(os.Stderr, "max-iters must be a positive integer")
		os.Exit(2)
	}
	src, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, "FAILED:", err)
		os.Exit(1)
	}
	root := repoRoot()
	h, err := openModule(filepath.Join(root, "target", "wasm32-unknown-unknown",
		"release", "active_wasm.wasm"))
	if err != nil {
		fmt.Fprintln(os.Stderr, "FAILED:", err)
		os.Exit(1)
	}
	defer h.close()
	type entry struct {
		Iter         int      `json:"iter"`
		Codes        []string `json:"codes"`
		Repaired     bool     `json:"repaired_by_resilient,omitempty"`
		Stuck        bool     `json:"stuck,omitempty"`
		Unrepairable bool     `json:"unrepairable,omitempty"`
	}
	var log []entry
	seen := map[string]bool{}
	current := string(src)
	fail := func(e error) {
		fmt.Fprintln(os.Stderr, "FAILED:", e)
		os.Exit(1)
	}
	for i := 1; i <= maxIters; i++ {
		env, err := h.evaluate(current)
		if err != nil {
			fail(err)
		}
		// Canonical policy, same as the Python/TypeScript loops:
		// the envelope's ok flag is the refusal signal (the bindings
		// raise exactly when ok is false). Diagnostics decide the
		// repair path: a present value with codes re-emits and
		// converges; only ok==false is unrepairable. Judging by JSON
		// presence instead over-repairs parse failures, whose error
		// envelopes may carry a partial payload.
		codes := codesOf(env.Diagnostics)
		e := entry{Iter: i, Codes: codes}
		key := strings.Join(codes, ",")
		if !env.OK {
			e.Unrepairable = true
			log = append(log, e)
			fail(fmt.Errorf("unrepairable: %v", codes))
		}
		if len(codes) == 0 {
			if len(env.JSON) == 0 || string(env.JSON) == "null" {
				e.Unrepairable = true
				log = append(log, e)
				fail(fmt.Errorf("clean codes but no value"))
			}
			log = append(log, e)
			out, _ := json.Marshal(map[string]any{"value": json.RawMessage(env.JSON), "log": log})
			fmt.Println(string(out))
			return
		}
		if seen[key] {
			e.Stuck = true
			log = append(log, e)
			fail(fmt.Errorf("stuck on codes %v", codes))
		}
		seen[key] = true
		e.Repaired = true
		log = append(log, e)
		next, err := reemit(env.JSON)
		if err != nil {
			fail(err)
		}
		current = next
	}
	fail(fmt.Errorf("budget exhausted after %d iterations", maxIters))
}
