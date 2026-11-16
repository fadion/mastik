//! The language against a plain document: the bar's sugar, name resolution,
//! collections, verbs, the header, budgets, completion and highlighting.

// A completion row's matched ranges are a list that often holds one.
#![allow(clippy::single_range_in_vec_init)]

use mastik_core::kurbo::{RoundedRectRadii, Size};
use mastik_core::verbs::{self, Ctx, Done};
use mastik_core::{Document, IdSource, NodeId, NodeKind, Resolved, Transaction};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::*;

/// A host over a plain document, committing as `verbs::run_script` does.
struct TestHost {
    doc: Document,
    res: Resolved,
    ids: IdSource,
    page: NodeId,
    commits: usize,
}

impl Host for TestHost {
    fn doc(&self) -> &Document {
        &self.doc
    }

    fn resolved(&self) -> &Resolved {
        &self.res
    }

    fn page(&self) -> NodeId {
        self.page
    }

    fn verb(&mut self, name: &str, args: &Value) -> Result<Done, String> {
        verbs::call(
            &mut Ctx {
                doc: &self.doc,
                res: &self.res,
                ids: &mut self.ids,
                images: None,
                contrast: None,
            },
            name,
            args,
        )
        .map_err(|e| e.to_string())
    }

    fn commit(&mut self, mut tx: Transaction) -> Result<(), String> {
        mastik_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc.apply(&tx).map_err(|e| e.to_string())?;
        self.res = Resolved::rebuild(&self.doc);
        self.commits += 1;
        Ok(())
    }
}

impl TestHost {
    /// A blank page.
    fn new() -> Self {
        let mut ids = IdSource::new(0x7E5B);
        let page = ids.mint();
        let doc = Document::new(page);
        let res = Resolved::rebuild(&doc);
        Self {
            doc,
            res,
            ids,
            page,
            commits: 0,
        }
    }

    /// Run `verb` and commit it; what it created.
    fn make(&mut self, verb: &str, args: Value) -> Vec<NodeId> {
        let done = self
            .verb(verb, &args)
            .unwrap_or_else(|e| panic!("{verb}: {e}"));
        let created = done.created.clone();
        self.commit(done.tx).unwrap();
        created
    }

    /// A `w`×`h` rect called `name` at `x`, `y` in `parent`.
    fn rect(&mut self, parent: NodeId, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        let kind = NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::from_single_radius(0.0),
            corner_smoothing: 0.0,
        };
        self.make(
            "create_node",
            json!({ "parent": parent.to_wire(), "kind": kind, "name": name, "x": x, "y": y }),
        )[0]
    }

    /// A frame called `name` on the page.
    fn frame(&mut self, name: &str, w: f64, h: f64) -> NodeId {
        let kind = NodeKind::Artboard {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        };
        let page = self.page;
        self.make(
            "create_node",
            json!({ "parent": page.to_wire(), "kind": kind, "name": name }),
        )[0]
    }

    /// The world bounds' left of `id`.
    fn x(&self, id: NodeId) -> f64 {
        self.res.world_bounds(id).unwrap().x0
    }

    /// The box width of `id`.
    fn width(&self, id: NodeId) -> f64 {
        mastik_core::query::local_box(&self.doc, &self.res, id)
            .unwrap()
            .width()
    }
}

/// Three rects 240, 260 and 300 wide at x 10, 50 and 90.
fn three() -> (TestHost, Vec<NodeId>) {
    let mut h = TestHost::new();
    let page = h.page;
    let ids = vec![
        h.rect(page, "A", 10.0, 0.0, 240.0, 40.0),
        h.rect(page, "B", 50.0, 100.0, 260.0, 40.0),
        h.rect(page, "C", 90.0, 200.0, 300.0, 40.0),
    ];
    (h, ids)
}

/// An env over `selection`, `labels`, no params.
fn env<'a>(selection: &'a [NodeId], labels: &'a FxHashMap<String, NodeId>) -> Env<'a> {
    Env {
        selection,
        labels,
        params: serde_json::Map::new(),
    }
}

/// Each `command-mode.md` §3 example (with registry names: `dup` and `rename` are not
/// verbs, so their lines stay as written) and the rule-3 operators.
#[test]
fn desugar_follows_the_four_rules() {
    let cases = [
        ("group", "group()"),
        ("align left", "align(\"left\")"),
        ("duplicate 3", "duplicate(3)"),
        ("duplicate { dx = 7 }", "duplicate { dx = 7 }"),
        ("align (\"left\")", "align (\"left\")"),
        ("set_visible false", "set_visible(false)"),
        ("set_name \"My Frame\"", "set_name(\"My Frame\")"),
        ("set_name my frame", "set_name(\"my\", \"frame\")"),
        ("set_visible false", "set_visible(false)"),
        ("dup 3", "dup 3"),
        (
            ".width = parent.width - 20",
            "sel.width = parent.width - 20",
        ),
        ("sk.x = .x + 5", "sk.x = sel.x + 5"),
        (".x += 15", "sel:adjust(\"x\", 15)"),
        (".x -= 4", "sel:adjust(\"x\", -4)"),
        (".x -= n * 2", "sel:adjust(\"x\", -(n * 2))"),
        (
            ".width *= 2",
            "sel:adjust(\"width\", function(v) return v * (2) end)",
        ),
        (
            ".width //= 3",
            "sel:adjust(\"width\", function(v) return v // (3) end)",
        ),
        ("sk.y += .height", "sk:adjust(\"y\", sel.height)"),
        (".width = 50%", "sel.width = percent(50)"),
        ("print(10 % 3)", "print(10 % 3)"),
        ("print(\".x\")", "print(\".x\")"),
        ("t[1].x", "t[1].x"),
        ("align(\"left\", n)", "align(\"left\", n)"),
        ("group = 3", "group = 3"),
        (".x += 5 -- nudge", "sel:adjust(\"x\", 5) -- nudge"),
        (
            ".x += 5; .y -= 2",
            "sel:adjust(\"x\", 5); sel:adjust(\"y\", -2)",
        ),
        (
            "for _, l in ipairs(sel) do l.x += 5 end",
            "for _, l in ipairs(sel) do l:adjust(\"x\", 5) end",
        ),
        (
            "if true then .x += 1 end",
            "if true then sel:adjust(\"x\", 1) end",
        ),
        (".width = 50% -- half", "sel.width = percent(50) -- half"),
    ];
    for (line, luau) in cases {
        assert_eq!(desugar(line).luau, luau, "{line}");
    }
    assert!(desugar(".x += 15").each_by_itself);
    assert!(!desugar(".x = 15").each_by_itself);
    for (line, kind) in [
        (".width", LineKind::Read),
        ("parent.width - 20", LineKind::Read),
        ("print(.x)", LineKind::Read),
        ("sel:shared(\"x\")", LineKind::Read),
        (".x = 1", LineKind::Write),
        (".x += 1", LineKind::Write),
        ("align left", LineKind::Write),
        ("group()", LineKind::Write),
        ("sel:select()", LineKind::Write),
        (".x == 1", LineKind::Read),
    ] {
        assert_eq!(desugar(line).kind, kind, "{line}");
    }
}

/// `marks` finds the subject's dots where rule 2 elides them and the free names,
/// never a name reached through `.` or one inside a string.
#[test]
fn marks_find_the_subject_and_free_names() {
    let line = ".x = fj.x - .x + print(\"sk\")";
    let m = marks(line);
    let names: Vec<&str> = m.names.iter().map(|r| &line[r.clone()]).collect();
    assert_eq!(
        m.subject,
        vec![0, 12],
        "the leading `.` and the one after `-`"
    );
    assert_eq!(names, vec!["fj", "print"], "no `x` after a dot, no string");
}

/// An unknown name raises naming it and the nearest real one, placed on it; a
/// global cannot be assigned; `os` and randomness are not there.
/// flip: `resolve` falling through to nil for an unknown name → fails at "No name".
#[test]
fn globals_are_strict() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let err = run_script(&mut h, &e, "local a = 1\nreturn selectoin.x").unwrap_err();
    assert_eq!(err.message, "No name “selectoin” — did you mean selection?");
    assert_eq!(err.line, Some(2));
    assert_eq!(err.span, Some(19..28));
    assert_eq!(err.fix, Some((19..28, "selection".to_owned())));
    let err = run_line(&mut h, &e, ".widht").unwrap_err();
    assert_eq!(err.message, "No property “widht” — did you mean width?");
    assert_eq!(err.span, Some(1..6), "placed in the line as typed");
    let err = run_script(&mut h, &e, "total = 5").unwrap_err();
    assert!(
        err.message.starts_with("No global “total”"),
        "{}",
        err.message
    );
    let err = run_script(&mut h, &e, "return os.time()").unwrap_err();
    assert!(err.message.starts_with("No name “os”"), "{}", err.message);
    let out = run_script(
        &mut h,
        &e,
        "return math.random == nil and math.floor(2.5) == 2",
    )
    .unwrap();
    assert_eq!(out.value.as_deref(), Some("true"));
    let err = run_script(&mut h, &e, "return require").unwrap_err();
    assert!(
        err.message.starts_with("No name “require”"),
        "{}",
        err.message
    );
}

/// A read across layers that disagree raises with the values and how to read
/// one or change each; one that agrees answers the value; `shared` answers nil.
/// flip: `props::agree` answering true → fails at the first `unwrap_err`.
#[test]
fn a_mixed_read_raises_naming_the_values() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let err = run_line(&mut h, &e, ".width").unwrap_err();
    assert_eq!(
        err.message,
        "The 3 layers have 3 widths (240, 260, 300) — .width += 20 adds to each; sel[1].width reads one"
    );
    let err = run_script(&mut h, &e, "return sel.width").unwrap_err();
    assert!(
        err.message
            .ends_with("sel:adjust(\"width\", 20) adds to each; sel[1].width reads one")
    );
    assert_eq!(
        eval_read(&mut h, &e, ".height").unwrap().as_deref(),
        Some("40")
    );
    assert_eq!(
        eval_read(&mut h, &e, "sel[2].width").unwrap().as_deref(),
        Some("260")
    );
    assert_eq!(
        eval_read(&mut h, &e, "sel:shared(\"width\")").unwrap(),
        None
    );
    assert_eq!(eval_read(&mut h, &e, "#sel").unwrap().as_deref(), Some("3"));
    assert_eq!(
        eval_read(&mut h, &e, "sel").unwrap().as_deref(),
        Some("A and 2 more")
    );
    assert_eq!(
        eval_read(&mut h, &e, "sel[1].name").unwrap().as_deref(),
        Some("\"A\"")
    );
}

/// `.x = 10` moves every layer's left to 10 in world space.
/// flip: `write` moving only `ids[0]` → fails at the second layer's x.
#[test]
fn a_write_broadcasts() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    run_line(&mut h, &e, ".x = 10").unwrap();
    for id in &ids {
        assert_eq!(h.x(*id), 10.0);
    }
    run_line(&mut h, &e, ".fill = \"#ff7a1a\"").unwrap();
    assert_eq!(
        eval_read(&mut h, &e, ".fill").unwrap().as_deref(),
        Some("\"#ff7a1a\"")
    );
    run_line(&mut h, &e, ".opacity = 0.3").unwrap();
    assert_eq!(
        eval_read(&mut h, &e, ".opacity").unwrap().as_deref(),
        Some("0.3")
    );
    run_line(&mut h, &e, ".rotation = 90").unwrap();
    assert_eq!(
        eval_read(&mut h, &e, ".rotation").unwrap().as_deref(),
        Some("90")
    );
}

/// Rule 3 and `adjust` change each layer by its own value.
/// flip: `adjust` writing `by` rather than `now + by` → fails at the x of A.
#[test]
fn adjust_changes_each_by_itself() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    run_line(&mut h, &e, ".x += 15").unwrap();
    assert_eq!([h.x(ids[0]), h.x(ids[1]), h.x(ids[2])], [25.0, 65.0, 105.0]);
    run_line(&mut h, &e, ".width *= 2").unwrap();
    assert_eq!(
        [h.width(ids[0]), h.width(ids[1]), h.width(ids[2])],
        [480.0, 520.0, 600.0]
    );
    run_script(
        &mut h,
        &e,
        "sel:adjust(\"x\", function(v) return v - 5 end)",
    )
    .unwrap();
    assert_eq!(h.x(ids[2]), 100.0);
    let err = run_line(&mut h, &e, ".fill += 1").unwrap_err();
    assert!(
        err.message.starts_with("`adjust` changes a number"),
        "{}",
        err.message
    );
}

/// A verb takes positional arguments after the subject the selection fills, a
/// collection first as its subject, or one table of named arguments.
/// flip: `call_verb` filling positional arguments from the subject parameter →
/// fails at `align left`'s unwrap.
#[test]
fn verbs_take_positional_and_named_arguments() {
    let (mut h, ids) = three();
    let mut labels = FxHashMap::default();
    labels.insert("sk".to_owned(), ids[2]);
    let e = env(&ids, &labels);
    run_line(&mut h, &e, "align left").unwrap();
    assert!(ids.iter().all(|id| h.x(*id) == 10.0), "aligned left");
    run_line(&mut h, &e, "set_opacity(sk, 0.5)").unwrap();
    assert_eq!(h.doc.get(ids[2]).unwrap().opacity(), 0.5);
    assert_eq!(
        h.doc.get(ids[0]).unwrap().opacity(),
        1.0,
        "only the collection given"
    );
    let out = run_line(&mut h, &e, "duplicate{dx = 7}").unwrap();
    assert_eq!(out.created.len(), 3);
    assert_eq!(h.x(out.created[0]), 17.0);
    let err = run_line(&mut h, &e, "align(\"left\", 1, 2)").unwrap_err();
    assert!(
        err.message.contains("takes 1 argument (edge)"),
        "{}",
        err.message
    );
    let err = run_line(&mut h, &e, "align sideways").unwrap_err();
    assert!(err.message.contains("edge"), "{}", err.message);
    let err = run_script(&mut h, &e, "local a = 1\n\nalign(\"sideways\")").unwrap_err();
    assert_eq!(
        err.line,
        Some(3),
        "the script's line, not the verb wrapper's"
    );
}

/// A verb answers what it created as a collection, and the run selects it.
#[test]
fn a_verb_returns_what_it_created() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let out = run_script(&mut h, &e, "local g = group()\nprint(#g, g.kind)\nreturn g").unwrap();
    assert_eq!(out.printed, ["1\tgroup"]);
    assert_eq!(out.created.len(), 1);
    assert_eq!(out.selection, Some(out.created.clone()));
    let group = out.created[0];
    assert_eq!(h.doc.get(group).unwrap().children(), ids.as_slice());
    assert_eq!(out.value, Some(h.doc.get(group).unwrap().name().to_owned()));
}

/// The header reads the documented example, and a bad line is an error on it.
#[test]
fn the_header_reads_inputs() {
    let source = "--- Card grid\n--- Lays cards out.\n---@input cols integer = 3 -- Columns {1..12}\n\
                  ---@input gap number = 24 -- Gap\n---@input fill color = #FFFFFF -- Card fill\n\
                  ---@input order choice(rows, columns) = rows -- Fill by\nlocal n = param.cols\n";
    let h = header(source).unwrap();
    assert_eq!(h.title.as_deref(), Some("Card grid"));
    assert_eq!(h.description, "Lays cards out.");
    let input = |name: &str, ty, default, label: &str, range, line| Input {
        name: name.into(),
        ty,
        default,
        label: Some(label.into()),
        range,
        line,
    };
    let choice = InputType::Choice(vec!["rows".into(), "columns".into()]);
    assert_eq!(
        h.inputs,
        [
            input(
                "cols",
                InputType::Integer,
                json!(3),
                "Columns",
                Some((1.0, 12.0)),
                3
            ),
            input("gap", InputType::Number, json!(24.0), "Gap", None, 4),
            input(
                "fill",
                InputType::Color,
                json!("#ffffff"),
                "Card fill",
                None,
                5
            ),
            input("order", choice, json!("rows"), "Fill by", None, 6),
        ]
    );
    let err = header("--- T\n---@input n intgr = 3\n").unwrap_err();
    assert_eq!(err.line, Some(2));
    assert_eq!(err.span, Some(18..23), "the type word");
    assert!(
        err.message.starts_with("No input type “intgr”"),
        "{}",
        err.message
    );
    let err = header("---@input cols integer = 30 -- Columns {1..12}").unwrap_err();
    assert!(err.message.contains("outside"), "{}", err.message);
}

/// Inputs reach the script as `param`, and an unknown one raises.
#[test]
fn param_holds_the_inputs() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let mut e = env(&ids, &labels);
    e.params.insert("cols".into(), json!(4));
    let out = run_script(&mut h, &e, "return param.cols * 2").unwrap();
    assert_eq!(out.value.as_deref(), Some("8"));
    let err = run_script(&mut h, &e, "return param.colz").unwrap_err();
    assert_eq!(err.message, "No input “colz” — did you mean cols?");
}

/// `while true do end` stops at the time budget with its own kind.
/// flip: the interrupt never reading the clock → fails at the message, the step
/// budget stopping it instead.
#[test]
fn a_runaway_loop_stops_at_the_budget() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let err = run_script(&mut h, &e, "while true do end").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Budget);
    assert_eq!(err.message, "Stopped after 1 s");
    let err = eval_read(&mut h, &e, "(function() while true do end end)()").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Budget);
    assert_eq!(
        err.message, "Stopped after 500000 steps",
        "a read's step budget"
    );
}

/// A live read refuses to write or call a verb, and the document stays as it was.
/// flip: `writable` answering Ok in `Mode::Read` → fails at the first `unwrap_err`.
#[test]
fn a_read_refuses_to_write() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let before = h.commits;
    let err = eval_read(&mut h, &e, ".x = 5").unwrap_err();
    assert!(
        err.message.starts_with("A live read changes nothing"),
        "{}",
        err.message
    );
    let err = eval_read(&mut h, &e, "align left").unwrap_err();
    assert!(
        err.message.starts_with("A live read changes nothing"),
        "{}",
        err.message
    );
    let err = eval_read(&mut h, &e, ".x += 5").unwrap_err();
    assert!(
        err.message.starts_with("A live read changes nothing"),
        "{}",
        err.message
    );
    assert_eq!(h.commits, before, "nothing committed");
    assert_eq!(h.x(ids[0]), 10.0);
}

/// Relative names, layer names and leap labels resolve; a duplicated layer name raises.
#[test]
fn names_resolve_in_order() {
    let mut h = TestHost::new();
    let frame = h.frame("Card", 400.0, 400.0);
    let a = h.rect(frame, "Title", 0.0, 0.0, 10.0, 10.0);
    let b = h.rect(frame, "Body", 0.0, 20.0, 10.0, 10.0);
    let mut labels = FxHashMap::default();
    labels.insert("sk".to_owned(), b);
    let sel = [a];
    let e = env(&sel, &labels);
    assert_eq!(
        eval_read(&mut h, &e, "parent.name").unwrap().as_deref(),
        Some("\"Card\"")
    );
    assert_eq!(
        eval_read(&mut h, &e, "next.name").unwrap().as_deref(),
        Some("\"Body\"")
    );
    assert_eq!(eval_read(&mut h, &e, "prev").unwrap(), None);
    assert_eq!(
        eval_read(&mut h, &e, "Card.children").unwrap().as_deref(),
        Some("Title and 1 more")
    );
    assert_eq!(
        eval_read(&mut h, &e, "sk.y").unwrap().as_deref(),
        Some("20")
    );
    assert_eq!(
        eval_read(&mut h, &e, "sk == next").unwrap().as_deref(),
        Some("true")
    );
    h.rect(frame, "Body", 0.0, 40.0, 10.0, 10.0);
    let err = eval_read(&mut h, &e, "Body.x").unwrap_err();
    assert!(
        err.message
            .starts_with("2 layers on this page are called “Body”"),
        "{}",
        err.message
    );
}

/// `50%` sizes a laid-out item through its layout item, and anywhere else says why not.
#[test]
fn percent_sizes_only_a_laid_out_layer() {
    let mut h = TestHost::new();
    let frame = h.frame("Row", 400.0, 100.0);
    let item = h.rect(frame, "Item", 0.0, 0.0, 40.0, 40.0);
    let loose = h.rect(h.page, "Loose", 0.0, 200.0, 40.0, 40.0);
    let labels = FxHashMap::default();
    let sel = [loose];
    let e = env(&sel, &labels);
    let err = run_line(&mut h, &e, ".width = 50%").unwrap_err();
    assert!(err.message.contains("takes no % width"), "{}", err.message);
    h.make(
        "set_display",
        json!({ "nodes": [frame.to_wire()], "display": { "Flex": {} } }),
    );
    let sel = [item];
    let e = env(&sel, &labels);
    run_line(&mut h, &e, ".width = 50%").unwrap();
    assert_eq!(
        h.doc.get(item).unwrap().item().width,
        mastik_core::container::Dimension::Percent(50.0)
    );
    assert_eq!(h.width(item), 200.0);
}

/// Completion offers verbs by prefix, a verb's choices once it is typed, and an
/// almost-match as a dim row with its suggestion.
#[test]
fn completion_offers_verbs_choices_and_suggestions() {
    let h = TestHost::new();
    let labels = FxHashMap::default();
    let c = complete(&h.doc, h.page, &labels, "al", 2);
    assert_eq!(c.replace, 0..2);
    let first = &c.rows[0];
    assert_eq!(
        (first.section, first.label.as_str()),
        (Section::Verbs, "align")
    );
    assert_eq!(first.matched, [0..2]);
    let c = complete(&h.doc, h.page, &labels, "align l", 7);
    assert_eq!(c.replace, 6..7);
    let words: Vec<&str> = c.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        words,
        ["left", "middle"],
        "the prefix first, then the subsequence"
    );
    assert!(c.rows.iter().all(|r| r.section == Section::Choices));
    let c = complete(&h.doc, h.page, &labels, ".alpha", 6);
    assert!(c.rows[0].dim);
    assert_eq!(c.rows[0].suggestion.as_deref(), Some("opacity"));
    let c = complete(&h.doc, h.page, &labels, ".wi", 3);
    assert_eq!(c.rows[0].label, "width");
    let c = complete(&h.doc, h.page, &labels, "up", 2);
    assert!(
        c.rows
            .iter()
            .any(|r| r.label == "tidy_up" && r.matched == [5..7])
    );
}

/// The editor's tokens tell Mastik's names from the rest.
#[test]
fn tokens_mark_the_api() {
    let src = "local w = sel.width -- note\nalign(\"left\", 2.5)";
    let got: Vec<(&str, Tok)> = tokens(src).into_iter().map(|(r, t)| (&src[r], t)).collect();
    assert_eq!(
        got,
        [
            ("local", Tok::Keyword),
            ("w", Tok::Plain),
            ("=", Tok::Plain),
            ("sel", Tok::Api),
            (".", Tok::Plain),
            ("width", Tok::Api),
            ("-- note", Tok::Comment),
            ("align", Tok::Api),
            ("(", Tok::Plain),
            ("\"left\"", Tok::String),
            (",", Tok::Plain),
            ("2.5", Tok::Number),
            (")", Tok::Plain),
        ]
    );
    assert_eq!(names_used(src), ["sel", "width", "align"]);
}

/// A lone `\r` ends a comment for the lexer as it does for Luau, and the code in
/// a backtick string's `{…}` is lexed as code: both run, so neither may be read
/// as a comment or a string.
/// flip: `line_end` stopping only at `\n` → fails at "the comment ends at `\r`".
#[test]
fn the_lexer_ends_lines_where_luau_does() {
    let src = "-- a\rb()";
    let got: Vec<(&str, Tok)> = tokens(src).into_iter().map(|(r, t)| (&src[r], t)).collect();
    assert_eq!(
        got,
        [
            ("-- a", Tok::Comment),
            ("b", Tok::Function),
            ("(", Tok::Plain),
            (")", Tok::Plain),
        ],
        "the comment ends at `\\r`"
    );
    assert_eq!(names_used("-- tidy\rsel.x = 5"), ["sel", "x"]);
    assert_eq!(names_used("local s = \"a\rgroup()"), ["group"]);
    let src = "print(`n {#sel} {group({})}`)";
    let strings: Vec<&str> = tokens(src)
        .into_iter()
        .filter(|(_, t)| *t == Tok::String)
        .map(|(r, _)| &src[r])
        .collect();
    assert_eq!(strings, ["`n {", "} {", "}`"], "the braces' code is code");
    assert_eq!(names_used(src), ["sel", "group"]);
    assert_eq!(
        as_read("-- a\rb()\r\nc = \"\u{202e}\0\"\t`\\u{41}`"),
        "-- a\nb()\nc = \"\\u{202e}\\u{0}\"\t`\\u{41}`",
        "the disclosure's view"
    );
    let src = "`\\u{41} {group()}`";
    assert_eq!(
        names_used(src),
        ["group"],
        "an escape's brace opens nothing"
    );
    // Luau agrees: the code after the `\r` runs.
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    run_script(&mut h, &env(&ids[..1], &labels), "-- tidy\rsel.x = 5").unwrap();
    assert_eq!(h.x(ids[0]), 5.0, "Luau ran the line");
}

/// The editor's tokens tell constants, the standard library and called names
/// from plain ones; a verb called stays Mastik's.
/// flip: `tokens` checking `calls` before `is_builtin` → fails at the list,
/// `ipairs` and `floor` read as calls.
#[test]
fn tokens_mark_constants_builtins_and_calls() {
    let src = "local function f(n) return nil end\nfor _, v in ipairs(sel) do f(math.floor(v.x)) end\nalign(\"left\", true)\nx.floor = math.pi";
    let got: Vec<(&str, Tok)> = tokens(src)
        .into_iter()
        .map(|(r, t)| (&src[r], t))
        .filter(|(_, t)| *t != Tok::Plain && *t != Tok::Keyword)
        .collect();
    assert_eq!(
        got,
        [
            ("f", Tok::Function),
            ("nil", Tok::Constant),
            ("ipairs", Tok::Builtin),
            ("sel", Tok::Api),
            ("f", Tok::Function),
            ("math", Tok::Builtin),
            ("floor", Tok::Builtin),
            ("x", Tok::Api),
            ("align", Tok::Api),
            ("\"left\"", Tok::String),
            ("true", Tok::Constant),
            ("math", Tok::Builtin),
            ("pi", Tok::Builtin),
        ]
    );
}

/// The labels of `complete_script`'s rows for `source` with the cursor at its
/// end, each with its section.
fn script_rows(source: &str) -> Vec<(Section, String)> {
    let (h, _) = three();
    complete_script(&h.doc, h.page, source, source.len())
        .rows
        .into_iter()
        .map(|r| (r.section, r.label))
        .collect()
}

/// A script's completion offers its own locals, functions, parameters and
/// loop variables, and Luau's keywords and library.
/// flip: `declared` skipping `for` → fails at "a loop variable".
#[test]
fn script_completion_offers_the_scripts_own_names() {
    let src = "local width_left, gap = 1, 2\nlocal function nudge(by, times)\n  for i, layer in ipairs(sel) do\n  end\nend\n";
    let has = |typed: &str, section: Section, label: &str| {
        script_rows(&format!("{src}{typed}")).contains(&(section, label.to_owned()))
    };
    assert!(has("wid", Section::Script, "width_left"), "a local");
    assert!(has("ga", Section::Script, "gap"), "the second local");
    assert!(has("nud", Section::Script, "nudge"), "a local function");
    assert!(has("tim", Section::Script, "times"), "a parameter");
    assert!(has("lay", Section::Script, "layer"), "a loop variable");
    assert!(has("fun", Section::Luau, "function"), "a keyword");
    assert!(has("ipa", Section::Luau, "ipairs"), "the library");
    assert!(has("se", Section::Names, "sel"), "the language's names");
    assert!(has("A", Section::Layers, "A"), "the page's layers");
    let typed = script_rows(&format!("{src}lay"));
    assert!(
        !typed.contains(&(Section::Script, "lay".to_owned())),
        "the word being typed is no name: {typed:?}"
    );
}

/// After a dot a script's completion offers a header's inputs on `param`, a
/// library's members on its name, and properties on anything else.
#[test]
fn script_completion_after_a_dot() {
    let header = "---@input gap number = 8 -- Gap\n---@input tint color = #ff0000\n";
    let rows = script_rows(&format!("{header}sel.x = param."));
    assert_eq!(
        rows,
        [
            (Section::Script, "gap".to_owned()),
            (Section::Script, "tint".to_owned())
        ]
    );
    assert_eq!(
        script_rows("local n = math.flo"),
        [(Section::Luau, "floor".to_owned())]
    );
    assert!(script_rows("sel.wid").contains(&(Section::Properties, "width".to_owned())));
    assert!(
        script_rows("sel:")
            .iter()
            .all(|(s, _)| *s == Section::Properties)
    );
}

/// A script's completion offers nothing in a string, in a comment, on a number,
/// or where a new name is being declared.
/// flip: `complete_script` without its `quoted` check → fails at "in a string".
#[test]
fn script_completion_keeps_quiet_where_no_name_goes() {
    assert_eq!(script_rows("print(\"se"), [], "in a string");
    assert_eq!(script_rows("local s = \"a\" -- se"), [], "in a comment");
    assert_eq!(script_rows("local se"), [], "naming a local");
    assert_eq!(script_rows("function se"), [], "naming a function");
    assert_eq!(script_rows("local n = 12"), [], "a number");
    assert!(
        !script_rows("print(\"a\") se").is_empty(),
        "after a closed string"
    );
}

/// Every library member highlighting and completion offer is one a script can
/// reach in the sandbox, every member a script can reach is offered, and
/// `math`'s random numbers are neither.
/// flip: `math.lerp` dropped from `LIBRARY` → fails at "math.lerp is listed".
#[test]
fn the_listed_library_is_the_sandboxs() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    for (lib, members) in crate::run::LIBRARY {
        assert!(crate::run::STDLIB.contains(lib), "{lib} is permitted");
        for m in *members {
            let out = run_script(&mut h, &e, &format!("return {lib}.{m} ~= nil"))
                .unwrap_or_else(|err| panic!("{lib}.{m}: {}", err.message));
            assert_eq!(out.value.as_deref(), Some("true"), "{lib}.{m} is there");
        }
        let out = run_script(
            &mut h,
            &e,
            &format!("for k in pairs({lib}) do print(k) end"),
        )
        .unwrap();
        assert!(!out.printed.is_empty(), "{lib} was walked");
        for m in &out.printed {
            assert!(members.contains(&m.as_str()), "{lib}.{m} is listed");
        }
    }
    assert!(!crate::run::library("math").contains(&"random"));
    let out = run_script(
        &mut h,
        &e,
        "return math.random == nil and table.getn == nil",
    )
    .unwrap();
    assert_eq!(
        out.value.as_deref(),
        Some("true"),
        "left out of the sandbox"
    );
}

/// The definitions describe the collection, the names and every verb.
#[test]
fn definitions_cover_the_api() {
    let d = definitions();
    assert!(d.contains("declare class Layers"));
    assert!(d.contains("    width: number\n"));
    assert!(d.contains("declare sel: Layers\n"));
    for verb in verbs::VERBS {
        assert!(
            d.contains(&format!("declare {}: ", verb.name)),
            "{}",
            verb.name
        );
    }
    assert!(
        d.contains("edge: (\"left\" | \"center\" | \"right\" | \"top\" | \"middle\" | \"bottom\")")
    );
}

/// `print` collects into the outcome, and a verb's own line joins it.
#[test]
fn print_collects_into_the_outcome() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let out = run_script(
        &mut h,
        &e,
        "for i, n in ipairs(sel) do print(i, n.name, n.x) end",
    )
    .unwrap();
    assert_eq!(out.printed, ["1\tA\t10", "2\tB\t50", "3\tC\t90"]);
}

/// Printing is held to `PRINT_CAP` bytes and `PRINT_LINES` lines, which Lua's
/// memory limit does not count: long lines and many empty ones both stop.
/// flip: `State::say` without its `PRINT_LINES` test → fails at "the line cap".
#[test]
fn printing_too_much_stops_the_run() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let src = "local s = string.rep('x', 400000) for i = 1, 10 do print(s) end";
    let err = run_script(&mut h, &e, src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Budget);
    assert_eq!(
        err.message,
        "Stopped after printing more than 10,000 lines or 1 MB"
    );
    let err =
        run_script(&mut h, &e, "for i = 1, 3e6 do print() end").expect_err("empty lines stop");
    assert_eq!(err.kind, ErrorKind::Budget);
    let out = run_script(&mut h, &e, "for i = 1, 10000 do print(i) end").unwrap();
    assert_eq!(out.printed.len(), 10_000, "the cap's own count is kept");
    let err = run_script(&mut h, &e, "for i = 1, 10001 do print(i) end").map(|o| o.printed.len());
    assert!(err.is_err(), "the line cap: {err:?}");
}

/// Luau's wording for a read-only table and for runaway recursion is said in the
/// app's terms.
#[test]
fn luau_internal_errors_read_plainly() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let err = run_script(&mut h, &e, "sel[1] = 5").unwrap_err();
    assert!(
        err.message.starts_with("This table cannot be changed"),
        "{}",
        err.message
    );
    let err = run_script(&mut h, &e, "local function f() return f() + 1 end\nf()").unwrap_err();
    assert_eq!(err.message, "A function called itself too deeply");
}

/// A Luau error lands on its line with the message alone.
#[test]
fn a_runtime_error_names_its_line() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let err = run_script(&mut h, &e, "local a = 1\nerror(\"boom\")").unwrap_err();
    assert_eq!((err.line, err.message.as_str()), (Some(2), "boom"));
    let err = run_script(&mut h, &e, "local a = \n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Error);
    assert!(err.line.is_some());
}

/// Every library member and permitted global function has a help line, and
/// every help line names one of them.
/// flip: a `LIB_DOCS` row removed → fails at "has a line".
#[test]
fn every_library_member_has_a_help_line() {
    let docs = crate::run::LIB_DOCS;
    let mut wanted: Vec<String> = Vec::new();
    for (lib, members) in crate::run::LIBRARY {
        wanted.extend(members.iter().map(|m| format!("{lib}.{m}")));
    }
    wanted.extend(
        crate::run::STDLIB
            .iter()
            .filter(|s| {
                crate::run::library(s).is_empty() && !matches!(**s, "math" | "string" | "table")
            })
            .map(|s| (*s).to_owned()),
    );
    for w in &wanted {
        assert!(docs.iter().any(|d| d.path == w), "{w} has a line");
    }
    for d in docs {
        assert!(wanted.iter().any(|w| w == d.path), "{} is a member", d.path);
        assert!(!d.doc.is_empty(), "{}", d.path);
    }
}

/// The hint lights the argument the caret is in, past nested calls, and says
/// when the arguments have run out.
/// flip: `call_at` keeping a closed bracket's frame → fails at "after a nested call".
#[test]
fn the_hint_lights_the_argument_the_caret_is_in() {
    let s = signature_at("align(", 6).unwrap();
    assert_eq!(s.name, "align");
    assert_eq!(s.params.len(), 1, "the subject is left out");
    assert_eq!(s.active, Some(0));
    assert_eq!(s.detail.as_deref(), Some("edge · which edge or centre"));
    assert!(s.doc.unwrap().ends_with(ACTS_ON_SELECTION));

    let src = "local copies = duplicate(0, ";
    let s = signature_at(src, src.len()).unwrap();
    assert_eq!(s.active, Some(1));
    assert_eq!(s.params[1].ty.as_deref(), Some("number?"));

    let src = "duplicate(math.floor(w / 2), ";
    assert_eq!(
        signature_at(src, src.len()).unwrap().active,
        Some(1),
        "after a nested call"
    );

    let src = "duplicate(1, 2, ";
    let s = signature_at(src, src.len()).unwrap();
    assert_eq!(
        (s.active, s.detail.as_deref()),
        (None, Some("No more arguments"))
    );

    assert_eq!(signature_at("align()", 7), None, "after the close");
    assert_eq!(signature_at("-- align(", 9), None, "in a comment");
}

/// A table argument lists what is left, and the script's own function shows
/// its parameter names and where it is declared.
#[test]
fn the_hint_covers_named_arguments_and_the_scripts_functions() {
    let src = "duplicate { dy = 8, ";
    let s = signature_at(src, src.len()).unwrap();
    assert!(s.named);
    assert_eq!(s.active, None);
    assert_eq!(s.detail.as_deref(), Some("Named: dx is left."));

    let src = "local function nudge(by, times)\nend\nnudge(4, ";
    let s = signature_at(src, src.len()).unwrap();
    assert_eq!(s.name, "nudge");
    assert_eq!(s.active, Some(1));
    assert_eq!(s.note.as_deref(), Some("This script's function · line 1"));
    assert_eq!(s.doc, None, "no doc is invented");

    let src = "sel:adjust(\"x\", ";
    let s = signature_at(src, src.len()).unwrap();
    assert_eq!((s.name.as_str(), s.active), ("sel:adjust", Some(1)));
}

/// Inside a choice parameter's quotes completion offers its words, quoted;
/// at a table argument's key, the names not yet written.
/// flip: `complete_script` returning nothing inside every string → fails at "quoted".
#[test]
fn completion_offers_choices_in_quotes_and_names_left() {
    let rows = script_rows("align(\"le");
    assert_eq!(
        rows.first(),
        Some(&(Section::Choices, "\"left\"".to_owned())),
        "quoted"
    );
    assert_eq!(
        script_rows("print(\"le"),
        [],
        "any other string stays quiet"
    );
    let rows = script_rows("duplicate { dy = 8, ");
    assert_eq!(rows, [(Section::Choices, "dx".to_owned())]);
}

/// A hover card says what a property, an input, a library member, a verb and
/// a layer are, and nothing for a plain local.
#[test]
fn hover_says_what_a_name_is() {
    let (h, _) = three();
    let at = |src: &str, word: &str| {
        let i = src.rfind(word).unwrap();
        hover_at(&h.doc, h.page, src, i)
    };
    let src = "for _, card in ipairs(sel) do card.opacity = 0.8 end";
    let c = at(src, "opacity").unwrap();
    assert_eq!(
        (c.title.as_str(), c.ty.as_deref()),
        ("card.opacity", Some("number"))
    );
    assert!(c.kind.starts_with("Property"));

    let src = "---@input gap number = 24 -- Gap\nprint(param.gap)";
    let c = at(src, "gap)").unwrap();
    assert_eq!(c.ty.as_deref(), Some("number = 24"));
    assert_eq!(c.doc.as_deref(), Some("Gap"));
    assert_eq!(c.kind, "This script's input, line 1");

    let c = at("local w = math.floor(3)", "floor").unwrap();
    assert_eq!(c.title, "math.floor(x: number)");
    assert_eq!(c.kind, "Luau library");

    let c = at("align(\"left\")", "align").unwrap();
    assert_eq!(c.kind, "Verb");

    let c = at("print(B.width)", "B.").unwrap();
    assert!(c.layer.is_some());
    assert_eq!(c.kind, "A layer named on this page");

    assert_eq!(at("local n = 3\nprint(n)", "n)"), None, "a plain local");
    assert_eq!(
        at("local B = 3\nprint(B)", "B)"),
        None,
        "a local hides the layer"
    );
}

/// The check marks a syntax error alone, with its line, and Run's hold reads
/// the same compile.
#[test]
fn a_syntax_error_is_found_without_running() {
    let src = "for i, card in ipairs(sel) do\n  card.x = i\n";
    let p = syntax_error(src).unwrap();
    assert_eq!(p.severity, Severity::Error);
    assert_eq!(p.line, 2, "the last line with text");
    assert_eq!(p.message, "Missing `end` to close the `do` on line 1");
    let (h, _) = three();
    assert_eq!(check(&h.doc, h.page, src), [p]);
    assert_eq!(syntax_error("local a = 1"), None);
}

/// A syntax error in the bar is placed on the token it names in the line as
/// typed, never the same letters in a string, and the commonest ones read
/// plainly; a run's unknown name is placed by the lexer the same way.
/// flip: `syntax_place` searching the line's text, strings included → fails at
/// "the `*` squiggled".
#[test]
fn a_bar_error_lands_on_the_code() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let line = "print(\"*\") .x = * 2";
    let err = run_line(&mut h, &e, line).unwrap_err();
    assert_eq!(err.span, Some(16..17), "the `*` squiggled: {}", err.message);
    let err = run_line(&mut h, &e, ".x = ").unwrap_err();
    assert_eq!(err.message, "Something is missing at the end");
    assert_eq!(err.span, Some(0..4), "the line's text");
    let err = run_line(&mut h, &e, "print(1").unwrap_err();
    assert_eq!(err.message, "Missing `)` to close `(`");
    let err = run_line(&mut h, &e, "print(\"selectoin\", selectoin.x)").unwrap_err();
    assert_eq!(err.span, Some(19..28), "the name, not the string");
    assert_eq!(err.fix.map(|f| f.0), Some(19..28));
}

/// Names the check cannot find are warnings with the run's fix; a wrong
/// property on layers, a missing library member or input and a global
/// assignment are errors; a layer on the page, a table's own key and a field of
/// a script's table are not marked.
/// flip: `check` skipping the bare-name arm → fails at "selectoin".
#[test]
fn the_check_marks_what_a_run_would_refuse() {
    let (h, _) = three();
    let first: Vec<String> = check(&h.doc, h.page, "sel.widht = 1")
        .into_iter()
        .map(|p| p.message)
        .collect();
    assert_eq!(first.len(), 1, "a dot on the text's first name");
    let src = "local n = #selectoin\nsel.widht = 320\nsel.fill = B.fill\n\
               local t = { size = 3 }\nt.foo = 1\nprint(math.flor(2))\ncount = 4\n\
               for _, c in ipairs(sel) do c.hieght = 1 end";
    let found: Vec<(Severity, String, Option<String>)> = check(&h.doc, h.page, src)
        .into_iter()
        .map(|p| (p.severity, p.message, p.fix.map(|f| f.1)))
        .collect();
    assert_eq!(
        found,
        [
            (
                Severity::Warning,
                "No name “selectoin” — did you mean selection?".to_owned(),
                Some("selection".to_owned())
            ),
            (
                Severity::Error,
                "No property “widht” — did you mean width?".to_owned(),
                Some("width".to_owned())
            ),
            (
                Severity::Error,
                "No “flor” in math — did you mean floor?".to_owned(),
                Some("floor".to_owned())
            ),
            (
                Severity::Error,
                "No global “count” — write `local count = …`".to_owned(),
                None
            ),
            (
                Severity::Error,
                "No property “hieght” — did you mean height?".to_owned(),
                Some("height".to_owned())
            ),
        ]
    );
}

/// Typed parameters and typed `local` lists declare every name, and the
/// library members Luau adds are known: the check marks nothing here.
/// flip: `declarations`' `list` not skipping a `: type` → fails at "nothing marked".
#[test]
fn typed_names_and_luaus_library_are_known() {
    let (h, _) = three();
    let src = "local function f(a: number, b: { number }, c: (number) -> string?)\n\
               \treturn math.lerp(a, b[1], 0.5), c\nend\n\
               local x: number, y: Map<string, number> = 1, {}\n\
               for i: number, v in ipairs(sel) do print(i, v, x, y) end\n\
               print(math.map(0.5, 0, 1, 0, 10), math.isnan(0), string.packsize(\"i4\"))";
    let found: Vec<String> = check(&h.doc, h.page, src)
        .into_iter()
        .map(|p| p.message)
        .collect();
    assert_eq!(found, Vec::<String>::new(), "nothing marked");
    // A return type ends before the body, whose names are still checked.
    let found: Vec<String> = check(
        &h.doc,
        h.page,
        "local function g(): number?\n\tzork() return 1\nend",
    )
    .into_iter()
    .map(|p| p.message)
    .collect();
    assert_eq!(found.len(), 1, "the body's name: {found:?}");
    assert!(found[0].starts_with("No name “zork”"), "{found:?}");
}

/// Go to declaration finds the nearest declaration above a use, a function
/// called above its own, and an input's header line; never a Mastik name.
#[test]
fn declaration_is_the_nearest_above() {
    let at = |src: &str, word: &str| {
        let i = src.rfind(word).unwrap();
        declaration_at(src, i).map(|(_, d)| d.start)
    };
    let src = "local a = 1\nlocal a = 2\nprint(a)";
    assert_eq!(at(src, "a)"), Some(18), "the second local");
    let src = "go()\nlocal function go() end";
    assert_eq!(at(src, "go()"), Some(20));
    let src = "---@input gap number = 24\nprint(param.gap)";
    assert_eq!(at(src, "gap)"), Some(10));
    assert_eq!(at("print(sel)", "sel"), None);
    let src = "local function f(by) return by end";
    assert_eq!(at(src, "by end"), Some(17), "a parameter");
    let src = "local function f(a: number, by: number) return by end";
    assert_eq!(at(src, "by end"), Some(28), "a typed parameter");
    let src = "local a: number, b = 1, 2\nprint(b)";
    assert_eq!(at(src, "b)"), Some(17), "a typed local list");
}
