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
