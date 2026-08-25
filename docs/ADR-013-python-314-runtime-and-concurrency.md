# ADR-013: Python 3.14 runtime and concurrency

**Status:** Accepted

**Date:** 2026-08-25

**Deciders:** Project maintainer

## Context

The production backend previously ran Python 3.12 and retained a Python 3.11 package and static
checking floor. That compatibility required split NumPy pins, a local NumPy typing shim, and the
stringized `from __future__ import annotations` behavior across most modules. The optional Essentia
TensorFlow release used by voice analysis did not publish a CPython 3.14 wheel at the time, so
moving to the newest Python line would have silently removed that feature.

Python 3.14 is now the current stable feature line and Essentia TensorFlow 2.1b6.dev1438 publishes
a CPython 3.14 Linux x86-64 wheel. Python 3.14 also makes deferred annotations native, adds
`InterpreterPoolExecutor`, officially supports the optional free-threaded build, and exposes an
experimental JIT. Those concurrency choices have different native-extension, isolation, memory,
and cancellation behavior and must not be treated as interchangeable version switches.

## Decision

Make CPython 3.14 the backend's minimum and production runtime, with the container pinned to the
current 3.14.7 maintenance image. CI resolves the current 3.14 patch through `setup-python`. Keep
all direct dependencies and build tooling pinned and continue using the locked transitive graph.

Adopt the language and typing facilities that improve this codebase without changing its runtime
contracts:

- use Python 3.14's native deferred annotations and remove the deprecated future-import mode;
- use PEP 695 `type` statements and generic function syntax where aliases and type variables
  already exist;
- use NumPy's real `NDArray` types and remove the Python 3.11 compatibility stub and split pin;
- target Python 3.14 in Ruff and MyPy so new code cannot silently reintroduce the older syntax
  and compatibility floor.

Retain the spawn-based `ProcessPoolExecutor` for whole-track analysis. It already provides true
multi-core execution, lets the parent own SQLite and durable job state, shares the cooperative
cancellation event, and contains failures in native audio or model code to worker processes.

## Options considered

### Free-threaded CPython and ordinary threads

Deferred. The free-threaded build is a separate ABI, not the default Python 3.14 runtime. Native
extensions can re-enable the GIL when they do not declare support, and the selected Essentia
TensorFlow release publishes a `cp314` wheel but no `cp314t` wheel. The free-threaded build also has
single-thread and memory overhead. Replacing the working process boundary would therefore remove
the voice feature or require maintaining a custom native stack before it could prove a benefit.

### `InterpreterPoolExecutor`

Deferred. Subinterpreters provide a GIL per interpreter, but their runtime state is isolated and
mutable objects cannot be shared. The current cancellation event, native module state, and
Essentia/TensorFlow loading path would need a new communication and compatibility design. They also
would not remove per-worker model state. Processes retain the clearer crash-containment boundary.

### Experimental CPython JIT

Rejected for production. The official slim image does not enable it by default, Python still
classifies it as experimental, and the analyzer's measured numeric hotspot already executes in
NumPy's native FFT and vector kernels. A custom interpreter build would add deployment risk before
production profiling shows a remaining Python-bytecode bottleneck.

### Continue supporting Python 3.11-3.13

Rejected. Production and CI would test a different language/runtime contract from the declared
minimum, the compatibility shim would mask NumPy's actual types, and the optional voice dependency
now gives the deployment a viable 3.14 upgrade path.

## Consequences

- Development and deployment now require Python 3.14 or newer; Python 3.11-3.13 environments fail
  package installation explicitly rather than running an untested compatibility path.
- Annotation introspection uses Python 3.14's deferred value semantics instead of strings. The full
  FastAPI, Pydantic, schema, and test suite is the compatibility gate for that change.
- The analyzer continues to use three independent processes in production; this decision does not
  change public analyzer IDs, stored evidence, or job contracts.
- Free-threading, subinterpreters, and the JIT remain measurable future options, not enabled claims.

## Action items

1. Compare the first production Python 3.14 profile with the prior run's stage totals and real-time
   factor.
2. Revisit free-threading only when NumPy, Essentia, TensorFlow, SQLAlchemy, Pydantic, and the server
   stack all publish and validate compatible free-threaded wheels.
3. Revisit interpreter workers only with a tested cancellation channel and native-extension
   isolation probe for the exact production dependency graph.
