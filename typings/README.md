# Dependency stubs

BasedPyright resolves dependency stubs from this directory. Keep each package in
its own subdirectory, following the installed package's module layout.

Generated snapshots can be used for dependency imports; correct the signatures
needed by the project against the installed dependency. The diagnostics hook
excludes files marked as generated and checks changed handwritten `.pyi` files
wherever they live in the Git worktree.
