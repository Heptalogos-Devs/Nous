# Maintained dependency stubs

BasedPyright resolves handwritten dependency stubs from this directory. Keep each
package in its own subdirectory, following the installed package's module layout.

Generated snapshots elsewhere under `typings/` remain available as references;
they do not replace the installed dependency's type information during imports.
The diagnostics hook still checks changed handwritten `.pyi` files wherever they
live in the Git worktree.
