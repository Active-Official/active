(* OCaml smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
   Handles cross the C stubs (ocaml_stubs.c) as nativeints, so the GC
   never chases raw C pointers. Ownership stays Model A: owned JSON is
   copied to an OCaml string and freed inside the stub; borrowed codes
   copy on return and are never freed; every handle is freed on all paths.
   Build: ocamlopt -o smoke ocaml_stubs.c smoke.ml <libactive_ffi.a> *)

external abi_version : unit -> int32 = "ml_abi"
external parse : string -> int -> nativeint = "ml_parse"
external module_free : nativeint -> unit = "ml_mod_free"
external check : nativeint -> nativeint = "ml_check"
external eval : nativeint -> nativeint = "ml_eval"
external diag_len : nativeint -> int = "ml_len"
external diag_code : nativeint -> int -> string = "ml_code"
external set_free : nativeint -> unit = "ml_set_free"
external value_free : nativeint -> unit = "ml_val_free"
external eval_json : nativeint -> string = "ml_eval_json"

let check_ok cond msg =
  if not cond then (prerr_endline ("SMOKE-FAIL: " ^ msg); exit 1)

let contains hay needle =
  let h = String.length hay and n = String.length needle in
  let rec loop i = i + n <= h && (String.sub hay i n = needle || loop (i + 1)) in
  loop 0

let with_module src f =
  let m = parse src (String.length src) in
  check_ok (m <> 0n) "parse returned null";
  (try f m with e -> module_free m; raise e);
  module_free m

let () =
  check_ok (abi_version () = 1l) "ABI major mismatch";
  print_endline "abi: 1 ok";
  with_module "port = 8080\n" (fun m ->
      let s = check m in
      check_ok (s <> 0n) "check returned null";
      let n = diag_len s in
      set_free s;
      check_ok (n = 0) "expected clean");
  print_endline "check clean ok";
  with_module "port = 8080\n" (fun m ->
      let v = eval m in
      check_ok (v <> 0n) "eval returned null";
      let json = eval_json v in
      value_free v;
      check_ok (contains json "8080") ("value missing: " ^ json));
  print_endline "eval port=8080 ok";
  with_module "timeout = 5s + 512MiB\n" (fun m ->
      let v = eval m in
      check_ok (v = 0n) "units mixed evaluated clean";
      let s = check m in
      check_ok (s <> 0n) "check returned null";
      let n = diag_len s in
      let code = if n > 0 then diag_code s 0 else "" in
      set_free s;
      check_ok (n > 0) "expected diagnostics";
      check_ok (code = "T001") ("expected T001, got " ^ code));
  print_endline "units refused ok";
  with_module "token = secret(\"hunter2\")\n" (fun m ->
      let v = eval m in
      check_ok (v <> 0n) "eval returned null";
      let json = eval_json v in
      value_free v;
      check_ok
        (contains json "***SECRET***" && not (contains json "hunter2"))
        ("redaction broken: " ^ json));
  print_endline "secret redacted ok";
  print_endline "SMOKE-OK"
