# Kubernetes service

Ships, replicas, resources, and probes in one validated document.
The schema leads (contract first), values follow: units are
first-class (`1GiB`, `30s`), bounds are enforced
(`replicas` 1–64, memory under 8 GiB):

```solid
{{#include ./kubernetes.solid}}
```

`solid check` fails the file the moment a bound breaks; `solid fmt`
normalizes either style; `solid graph` shows the (shallow here)
dependency shape. Profiles and includes scale this file to fleets
without templating languages.
