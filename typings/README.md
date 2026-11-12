# Dependency stubs

BasedPyright resolves dependency stubs from this directory. Keep each package in
its own subdirectory, following the installed package's module layout.

Generated snapshots supply dependency types. BasedPyright ignores
`typings/pypdfium2`, and Ruff does not lint it, so snapshot diagnostics stay
out of normal checking and are not reported against project code. The
diagnostics hook still skips generator-marked files and checks other changed
handwritten `.pyi` files.
