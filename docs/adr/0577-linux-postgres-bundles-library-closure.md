# 0577: Linux PostgreSQL bundles its non-glibc library closure

- **Status**: Accepted
- **Date**: 2026-10-26

## Context

The relocatable PostgreSQL 18.4.0 Linux archives depend on host libraries.
Ubuntu 26.04 lacks the libxml2.so.2 SONAME required by postgres, preventing
workspace startup while the gateway remains healthy. OpenSSL, Kerberos,
compression libraries, readline, and optional extensions add further dependencies.
Both Linux architectures use `stage_runtime_fetch_postgres`.

The macOS archive uses Apple's `/usr/lib/libxml2.2.dylib` and bundles OpenSSL
with loader-relative paths. The Linux host contract cannot assume equivalent
system libraries across distributions.

## Decision

Bundle every non-glibc ELF dependency recursively from the Ubuntu 22.04 build
baseline. Give every ELF consumer an origin-relative search path to `postgres/lib`.
Check the staged tree before tarball emission, independently of host library availability.

## Rationale

Bundling preserves PostgreSQL's compiled features and catches dependencies beyond
libxml2. Each library needs its own search path because ELF RUNPATH does not
apply transitively. Static metadata inspection also works for either architecture
without executing PostgreSQL during the dependency audit.

## Consequences

Linux builds require `readelf`, `patchelf`, `ldconfig`, and installed dependency
packages. The release workflow installs them for both architectures. Copied
libraries retain their package copyright notices under `postgres/share/licenses`.

The closure gate rejects missing libraries, escaping symlinks, wrong ELF
architectures, host search paths, and GLIBC requirements above 2.35. Cached
PostgreSQL trees undergo collection and validation too. glibc and its loader
remain host supplied. macOS staging stays unchanged.

The check covers ELF dependencies. It does not establish completeness of
optional language runtimes, dynamically discovered plugins, or external configuration.

## Alternatives considered

- Rebuild without XML: changes PostgreSQL features and leaves other host dependencies unresolved.
- Require host libxml2 packages: defeats the self-contained installation contract and varies by distribution.
- Copy only libxml2: leaves transitive libraries and other direct dependencies host supplied.
- Set LD_LIBRARY_PATH in launchers: spreads packaging policy across launchers and their child processes.
