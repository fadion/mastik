# Mastik — the assistant

> **Status: what is left.** The assistant itself is built and its design is in `design/assistant.md`.
> This file keeps only what is still unbuilt; when a part is built, its rules move there. A `§n`
> cited against this file is a section of `design/assistant.md`, which keeps the numbers.

## `/ai`

Command Mode's `/ai` is a later door into the same assistant: a request typed in the command bar
(`design/command-mode.md`) goes to the configured harness, and what it proposes lands as one
undoable step. The bar is built; `/ai` in it is not.

## Script blocks in replies

A Luau code block in a reply is to gain *Open in script editor* (the editor is built,
`design/command-mode.md` §10). Until then it renders as any other code block.

## Open questions

1. **`/ai`'s look in the bar**: how a request and its proposed script show there. Its mechanics are
   the assistant's: the configured harness, its sign-in, its streaming.
