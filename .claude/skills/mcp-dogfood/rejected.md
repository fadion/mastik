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
- **A DTCG `lineHeight` written as `"64px"`.** The DTCG format defines a typography token's
  `lineHeight` as a number, the multiplier; the refusal says so.
