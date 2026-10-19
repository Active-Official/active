// W-005 wazero smoke: pure-Go embedding of active_wasm.wasm with zero
// imports. Evaluates a fixture through the raw exports and asserts the
// outcome. This is the no-cgo path proof; B-002 builds the real active-go
// repo on top of it.
package main

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"

	"github.com/tetratelabs/wazero"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "SMOKE-FAIL:", err)
		os.Exit(1)
	}
}

func run() error {
	ctx := context.Background()
	rt := wazero.NewRuntime(ctx)
	defer rt.Close(ctx)

	wasm, err := os.ReadFile("../../target/wasm32-unknown-unknown/release/active_wasm.wasm")
	if err != nil {
		return err
	}
	mod, err := rt.InstantiateWithConfig(ctx, wasm, wazero.NewModuleConfig())
	if err != nil {
		return err
	}
	mem := mod.Memory()
	alloc := mod.ExportedFunction("active_alloc")
	eval := mod.ExportedFunction("active_eval_json")
	free := mod.ExportedFunction("active_free")
	abi := mod.ExportedFunction("active_abi_version")

	write := func(s string) (uint32, uint64) {
		b := []byte(s)
		out, err := alloc.Call(ctx, uint64(len(b)))
		if err != nil || len(out) == 0 {
			panic("alloc: " + fmt.Sprint(err))
		}
		ptr := uint32(out[0])
		if !mem.Write(ptr, b) {
			panic("write OOB")
		}
		return ptr, uint64(len(b))
	}
	read := func(ptr uint32, n uint64) []byte {
		b, ok := mem.Read(ptr, uint32(n))
		if !ok {
			panic("read OOB")
		}
		return b
	}
	u32le := func(ptr uint32) uint64 {
		b := read(ptr, 4)
		return uint64(binary.LittleEndian.Uint32(b))
	}

	abiOut, err := abi.Call(ctx)
	if err != nil || len(abiOut) == 0 || abiOut[0] != 1 {
		return fmt.Errorf("abi version: %v %v", abiOut, err)
	}
	fmt.Println("abi: 1")

	sp, sn := write("port = 8080\n")
	slot, _ := write("\x00\x00\x00\x00")
	out, err := eval.Call(ctx, uint64(sp), sn, uint64(slot))
	if err != nil {
		return fmt.Errorf("eval trap: %w", err)
	}
	envLen := u32le(slot)
	var env struct {
		Ok   bool           `json:"ok"`
		JSON map[string]any `json:"json"`
		Diag string         `json:"diagnostics"`
	}
	if err := json.Unmarshal(read(uint32(out[0]), envLen), &env); err != nil {
		return err
	}
	free.Call(ctx, uint64(sp), sn)
	free.Call(ctx, out[0], envLen)
	free.Call(ctx, uint64(slot), 4)
	if !env.Ok || env.JSON["port"] != 8080.0 {
		return fmt.Errorf("unexpected envelope: %+v", env)
	}
	fmt.Println("eval port=8080 ok")

	sp2, sn2 := write("timeout = 5s + 512MiB\n")
	slot2, _ := write("\x00\x00\x00\x00")
	out2, err := eval.Call(ctx, uint64(sp2), sn2, uint64(slot2))
	if err != nil {
		return fmt.Errorf("eval trap: %w", err)
	}
	envLen2 := u32le(slot2)
	var env2 struct {
		Ok   bool   `json:"ok"`
		Diag string `json:"diagnostics"`
	}
	if err := json.Unmarshal(read(uint32(out2[0]), envLen2), &env2); err != nil {
		return err
	}
	if env2.Ok {
		return fmt.Errorf("broken units evaluated clean")
	}
	fmt.Println("units refused ok")
	return nil
}
