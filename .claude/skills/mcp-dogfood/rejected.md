# Rejected findings

Findings the loop judged and declined, one line each: the finding, then why it does not hold. A
run that reports one of these again does not count it as meaningful. A rejection that is a design
decision belongs in `docs/non-goals.md` or `docs/decisions.md` instead.

- **A script given a short id cannot split it on `:`.** Short ids in a script's source are
  expanded to full ids (`"<actor>:<seq>"`) before it runs; a script that joins an id to another
  value with `:` and splits it again gets the actor alone. The script's own parsing, not the tools.
- **Scripts lack `pcall`, and reading a group's mixed `fills` fails.** Command Mode leaves out
  `pcall` by design (`command-mode.md`), and `sel:shared(name)` is the read that answers nil for
  layers that differ.
- **`place_asset` as a `batch` step, or with an `index`.** It waits on the network, which a
  batch's one undo step cannot hold; a `reparent` with `index` after it places the icon first.
- **Every bad path of a model value in one refusal.** A value is read whole and refused at the
  first place serde stops, with that place's form; a second error behind it may not exist once
  the first is put right.
- **Photo thumbnails not labelled with their asset.** Each thumbnail follows a text naming its
  asset id, and `said` says where thumbnails stop at the answer's 64 KB.
- **A boolean of a stroked path cuts with its stroke.** Booleans combine fill geometry, as every
  design tool's do; `outline_stroke` turns a stroke into a shape a boolean cuts with.
- **A rate-limited photo download retried.** The service's limit is the person's; the refusal
  says the source answered HTTP 429, and another photo is the way on.
- **An instance root following its main's layout item.** An instance root's placement is its
  own (`SetLayoutItem` among it, `components.md`), as its transform is.
- **`fill` as a layout item's width or height.** It has no one meaning: on the main axis it is
  `grow`, across it a stretch (`align_self`), and neither is a size.
- **`id` for the checks' `ids`.** The refusal names the parameters the tool takes; a second name
  for the same list is one more thing every agent reads.
- **A seam where two frames meet in a page drawn below 1×.** Two antialiased edges on one
  fractional pixel each cover it partly, as in any rasteriser; a region export at 2× shows the page
  as it is.
- **A DTCG `lineHeight` written as `"64px"`.** The DTCG format defines a typography token's
  `lineHeight` as a number, the multiplier; the refusal says so.
- **Resetting an instance root resets only that root.** Resetting an instance puts the whole
  instance back as its main has it, as the menu's *Reset all* does; a layer inside it is reset
  by naming that layer.
- **A frame stretched across its flex line by `align_self`.** A frame's `Auto` size is its
  stored size, which a stretch never changes, as CSS stretches only an `auto` cross size; a
  frame fills by `{"Percent": 100}`, which the refusal of a `Fill` width now names.
- **A 1px flex gap drawn crisp at every row.** A gap at a fractional offset covers two pixels
  partly, as in any rasteriser, as the seam rejected above.
- **A tree copy's children matched by name rather than by `copy_of`.** A copy written without
  children takes its main's whole; one giving children names each one's source, since two of a
  main's layers may share a name.
- **`@@` collapsed anywhere in a batch string.** Only a string starting with `@` is a reference,
  and the batch's `steps` says to double that leading `@`; a mid-string `@@` stays as written.
- **A text layer showing an expression such as `{Tasks.opened.Title}`.** The opened row is
  shown by a list whose rows are `opened`, filling a component's text properties
  (`lists.md`); braces in a text are its content, which a person may well write.
- **A list of every global a stored script can reach.** Command Mode's names and their order
  are its design (`command-mode.md`); the Luau sandbox leaves out `_G` and `getfenv` on purpose.
- **A "self" target for *Change to* across several layers.** `set_reactions` writes the same
  reactions to every layer it names; layers whose targets differ take a call each.
- **A script's colour write keeping the variable it equals.** A written value replaces its
  binding, as the inspector's does; a matching colour is not a binding.
- **`get_outline` naming an instance's kind as `instance`.** The outline's `kind` is the
  layer's kind, which agents compare against; `get_node` and `list_components` give the role.
- **`print` showing a Luau table's contents.** Luau prints a table as its address; that is
  the language's, not a tool's.
