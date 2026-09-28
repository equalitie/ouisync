# Ouisync gradle convention plugin

Gradle convention plugin that applies some common properties to projects:

- Sets the `ndkVersion` property on android applications and android libraries. The version is read
  from a file named `ndk.version` which it looks for in the current project directory and all
  parent directories (uses the the first one found).


It also provides helpers for building rust crates:

- `CargoBuild` task which builds a dynamic library from a rust crate for a single target (using
  `cargo` or `cross`) and copies it into a given output directory.
- `RustTarget` with utilities for working with target triples (host target detection, library file
  names, JNA resource prefixes).
