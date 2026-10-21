# Elixir smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# A minimal NIF (active_nif.c) wraps the frozen C ABI: module handles
# live in a resource type with a destructor, codes come back as binaries,
# eval as a binary or :refused. This driver asserts the smoke contract.
defmodule ActiveNif do
  @on_load :load_nif
  def load_nif do
    # :erlang.load_nif appends the shared-library extension itself,
    # so ACTIVE_NIF_SO is the path WITHOUT .so.
    path = System.get_env("ACTIVE_NIF_SO") || raise "set ACTIVE_NIF_SO"
    :erlang.load_nif(String.to_charlist(path), 0)
  end

  def abi, do: :erlang.nif_error(:nif_not_loaded)
  def parse(_src), do: :erlang.nif_error(:nif_not_loaded)
  def check(_mod), do: :erlang.nif_error(:nif_not_loaded)
  def eval_json(_mod), do: :erlang.nif_error(:nif_not_loaded)
end

defmodule Smoke do
  def check!(cond, msg) do
    unless cond do
      IO.puts(:stderr, "SMOKE-FAIL: #{msg}")
      System.halt(1)
    end
  end

  def run do
    check!(ActiveNif.abi() == 1, "ABI major mismatch")
    IO.puts("abi: 1 ok")

    mod = ActiveNif.parse("port = 8080\n")
    check!(is_reference(mod), "parse returned error")
    codes = ActiveNif.check(mod)
    check!(is_list(codes) and codes == [], "expected clean")
    IO.puts("check clean ok")

    json = ActiveNif.eval_json(mod)
    check!(is_binary(json), "eval returned refusal")
    check!(String.contains?(json, "8080"), "value missing: #{json}")
    IO.puts("eval port=8080 ok")

    bad = ActiveNif.parse("timeout = 5s + 512MiB\n")
    check!(ActiveNif.eval_json(bad) == :refused, "units mixed evaluated clean")
    codes = ActiveNif.check(bad)
    check!(is_list(codes) and codes == ["T001"], "expected T001, got #{inspect(codes)}")
    IO.puts("units refused ok")

    sec = ActiveNif.parse("token = secret(\"hunter2\")\n")
    sjson = ActiveNif.eval_json(sec)
    check!(is_binary(sjson), "eval returned refusal")
    check!(
      String.contains?(sjson, "***SECRET***") and not String.contains?(sjson, "hunter2"),
      "redaction broken: #{sjson}"
    )
    IO.puts("secret redacted ok")
    IO.puts("SMOKE-OK")
  end
end

Smoke.run()
