# Terraform variables

Typed inputs with defaults and bounds — `variable` blocks with
teeth. A wrong type or an out-of-range count fails `solid check`
before anything provisions:

```solid
{{#include ./terraform-vars.solid}}
```
