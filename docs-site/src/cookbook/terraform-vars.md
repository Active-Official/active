# Terraform variables

Typed inputs with defaults and bounds — `variable` blocks with
teeth. A wrong type or an out-of-range count fails `ACTIVE check`
before anything provisions:

```active
{{#include ./terraform-vars.active}}
```
