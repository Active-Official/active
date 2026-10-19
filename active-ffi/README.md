# active-ffi

C ABI for embedding Active. Opaque handles, panic isolation on every
export, structured diagnostics, budgets enforced inside the core, secret
discipline for hosts. The normative contract is `docs/FFI.md`; this crate
is its implementation.

**Active is the configuration language that refuses to lie.** What you
write is what runs. Units have dimensions; secrets have taint; budgets
always hold.
