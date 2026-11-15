# Mastik — the scripting VM

> **Status: what is left.** Command Mode is built, and its design is in `design/command-mode.md`,
> which keeps this file's section numbers: a `vm.md §n` cited from code is that section there. This
> file keeps only what is still open.

## Reaching outside the document — parked

The case: one base design in twenty colour-and-logo variations, each its own file. That needs
writing files and reading images, which the sandbox forbids (`design/command-mode.md` §8). The
answer today is one document: the script generates the frames, or a variable mode per brand with
each frame exported. If files are wanted, two shapes are on the table and neither is ruled:

- **Host-mediated**: the script returns new documents and Mastik saves them into a folder the person
  picks after the run; images arrive as data from a folder the person picks, never as paths.
- **A CLI batch run**: `mastik run script.luau --in base.mastik --out dir/`, the shell owning the
  files.

It is its own decision because it is the first time a script reaches past the document.
