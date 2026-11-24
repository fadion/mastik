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
    /// The last commit's inverse.
    last: Option<Transaction>,
    /// How long each verb takes, as a slow host's might.
    slow: std::time::Duration,
    /// How many verbs were called.
    verbs: usize,
    /// Set by each verb, as the person pressing Escape while one runs.
    stop_on_verb: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
        std::thread::sleep(self.slow);
        self.verbs += 1;
        if let Some(stop) = &self.stop_on_verb {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
        let applied = self.doc.apply(&tx).map_err(|e| e.to_string())?;
        self.last = Some(applied.inverse);
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
            last: None,
            slow: std::time::Duration::ZERO,
            verbs: 0,
            stop_on_verb: None,
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
        (
            "local v = 2; .width *= v",
            "local v = 2; sel:adjust(\"width\", function(v_) return v_ * (v) end)",
        ),
        (
            ".x += 5 .y += 3",
            "sel:adjust(\"x\", 5) sel:adjust(\"y\", 3)",
        ),
        (
            "sk.x += 5 sk.y -= 1",
            "sk:adjust(\"x\", 5) sk:adjust(\"y\", -1)",
        ),
        (
            "print(`{.name} is {.width} wide`)",
            "print(`{sel.name} is {sel.width} wide`)",
        ),
        ("print(f \"s\" .x)", "print(f \"s\" .x)"),
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
        (".name ..= \" copy\"", LineKind::Write),
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
    assert_eq!(
        marks("print(`{.x}`)").subject,
        vec![8],
        "an interpolation's `{{` is an opening bracket"
    );
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
    // Nothing near: the properties there are, so the next guess is not blind.
    let err = run_line(&mut h, &e, ".text_style").unwrap_err();
    assert!(
        err.message.contains("a layer has") && err.message.contains("font_family"),
        "{}",
        err.message
    );
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
    // flip: `run` copying the value as shown → fails at "copied bare".
    let read = read_line(&mut h, &e, "sel[1].name").unwrap();
    assert_eq!(read.copied.as_deref(), Some("A"), "copied bare");
    let read = run_line(&mut h, &e, "sel[2].width").unwrap();
    assert_eq!(read.copied, read.value, "a number copies as shown");
}

/// A finite number near `f64::MAX` reads as its digits, never as "inf".
/// flip: `format_number` rounding every finite number → fails at "1e306".
#[test]
fn a_huge_number_reads_whole() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let read = eval_read(&mut h, &e, "1e306").unwrap().unwrap();
    assert!(
        read.starts_with("1000") && read.len() == 307,
        "1e306: {read}"
    );
    assert_eq!(format_number(-2.5e15), "-2500000000000000");
    assert_eq!(format_number(1.23456), "1.235");
    assert_eq!(
        eval_read(&mut h, &e, "math.huge").unwrap().as_deref(),
        Some("inf")
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
    run_line(&mut h, &e, "local v = 2; .width /= v").unwrap();
    assert_eq!(h.width(ids[0]), 240.0, "the amount's `v` is the script's");
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

/// A plain list of layers is a verb's subject too, as a collection is: each
/// layer once, the rest of the arguments after it.
/// flip: `layers_of` taking collections alone → the run is refused, the list
/// read as one argument too many.
#[test]
fn a_list_of_layers_is_a_verbs_subject() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let src = "local a, b = sel[1], sel[2]\nset_opacity({a, b, a}, 0.5)";
    run_script(&mut h, &e, src).unwrap();
    assert_eq!(
        [0, 1, 2].map(|i| h.doc.get(ids[i]).unwrap().opacity()),
        [0.5, 0.5, 1.0],
        "the list's layers"
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
    // A verb of one layer runs once for each, and what every run made is selected.
    // flip: `call_verb` keeping only the last run's creations → fails here.
    let frames = [h.frame("F", 100.0, 100.0), h.frame("G", 100.0, 100.0)];
    let out = run_line(&mut h, &env(&frames, &labels), "create_text(\"hi\")").unwrap();
    assert_eq!(out.created.len(), 2, "a text in each frame");
    assert_eq!(out.selection, Some(out.created.clone()), "both selected");
}

/// A script makes a colour scale with the registry's `generate_scale`, one
/// table of named arguments and no layer for a subject.
#[test]
fn a_script_generates_a_colour_scale() {
    let mut h = TestHost::new();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    run_script(
        &mut h,
        &e,
        "local v = add_variable{kind = \"color\", name = \"brand\", value = {color = \"#3366cc\"}}\n\
         generate_scale{variable = v}",
    )
    .unwrap();
    let table = h.doc.variables();
    for step in mastik_core::oklch::SCALE_STEPS {
        assert!(table.by_name(&format!("brand/{step}")).is_some(), "{step}");
    }
}

/// A verb's arguments hold a bounded count of values, so a table sharing one
/// subtable at every level is refused rather than expanded as a tree, and a table
/// inside itself is refused at its depth.
/// flip: `to_json` never spending `budget` → fails at "more than 100000 values".
#[test]
fn a_verb_argument_is_bounded() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let shared = "local t = 0 for i = 1, 18 do t = {t, t} end\n";
    for call in ["set_name { name = t }", "set_name(t)"] {
        let err = run_script(&mut h, &e, &format!("{shared}{call}")).unwrap_err();
        assert!(
            err.message.contains("more than 100000 values"),
            "{call}: {}",
            err.message
        );
    }
    let err = run_script(&mut h, &e, "local t = {} t[1] = t set_name(t)").unwrap_err();
    assert!(err.message.contains("nests too deep"), "{}", err.message);
    assert_eq!(h.commits, 3, "nothing past the fixture was committed");
}

/// A table repeating one long string or one collection is small in Lua, so a
/// verb's arguments are charged each string's bytes and each collection's layers
/// before the host copies them.
/// flip: `to_json` charging a string no bytes → fails at "more than 16 MB";
/// charging a collection one value → fails at "more than 100000 values".
#[test]
fn a_repeated_argument_is_bounded() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let strings = "set_name { name = table.create(1000, string.rep(\"x\", 1e5)) }";
    let err = run_script(&mut h, &e, strings).unwrap_err();
    assert!(err.message.contains("more than 16 MB"), "{}", err.message);
    let layers = "set_name { name = table.create(40000, sel) }";
    let err = run_script(&mut h, &e, layers).unwrap_err();
    assert!(
        err.message.contains("more than 100000 values"),
        "{}",
        err.message
    );
    assert_eq!(h.commits, 3, "nothing past the fixture was committed");
}

/// A verb of one layer hands its arguments to the host once per layer, and a
/// property write once per layer, so the bytes bound counts them per layer.
/// flip: `call_verb` not multiplying its arguments' bytes by its layers → fails
/// at the call form; `write` not multiplying its text's → at the property form.
#[test]
fn a_long_argument_over_many_layers_is_bounded() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    for src in [
        "set_name(string.rep(\"x\", 1e7))",
        "sel.name = string.rep(\"x\", 1e7)",
    ] {
        let err = run_script(&mut h, &e, src).unwrap_err();
        assert!(
            err.message.contains("more than 16 MB"),
            "{src}: {}",
            err.message
        );
    }
    assert_eq!(h.commits, 3, "nothing past the fixture was committed");
    run_script(
        &mut h,
        &env(&ids[..1], &labels),
        "set_name(string.rep(\"x\", 1e7))",
    )
    .expect("one layer takes it");
    assert_eq!(h.doc.get(ids[0]).unwrap().name().len(), 10_000_000);
}

/// A verb run once per layer, and a property written to each, read the clock
/// between layers, so a slow host over a large selection stops near the budget.
/// flip: `call_verb` not reading the clock between its runs → fails at the call
/// form's count; `write` not reading it → fails at the property form's.
#[test]
fn a_verb_over_many_layers_is_timed_per_layer() {
    let mut h = TestHost::new();
    let page = h.page;
    let many: Vec<NodeId> = (0..80)
        .map(|i| h.rect(page, &format!("R{i}"), 0.0, 0.0, 10.0, 10.0))
        .collect();
    let labels = FxHashMap::default();
    let e = env(&many, &labels);
    h.slow = std::time::Duration::from_millis(30);
    for src in ["set_name(\"x\")", "sel.name = \"x\""] {
        let before = h.verbs;
        let err = run_script(&mut h, &e, src).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Budget, "{src}: {}", err.message);
        let called = h.verbs - before;
        assert!(called < 45, "{src}: {called} verbs ran past the 1 s budget");
    }
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
    // A quoted default may hold ` --`; the label starts after it.
    let h = header("---@input sep string = \"a -- b\" -- Separator").unwrap();
    assert_eq!(h.inputs[0].default, json!("a -- b"));
    assert_eq!(h.inputs[0].label.as_deref(), Some("Separator"));
    // A default has no escapes, so one ending in `\` closes at its next quote.
    // flip: `quoted_end` skipping the byte after `\` → fails at the default.
    let h = header("---@input dir string = \"C:\\\" -- Folder \"out\"").unwrap();
    assert_eq!(h.inputs[0].default, json!("C:\\"));
    assert_eq!(h.inputs[0].label.as_deref(), Some("Folder \"out\""));
}

/// Each header refusal says why and underlines the text it is about.
/// flip: the duplicate-name refusal underlining the input's text after `@input`
/// rather than its whole line → fails at "Two inputs".
#[test]
fn the_header_refuses_with_a_span() {
    let cases = [
        (
            "---@inptu cols integer = 3",
            "No header tag “@inptu”",
            "@inptu",
        ),
        (
            "---@input a number\n---@input a number",
            "Two inputs are called “a”",
            "---@input a number",
        ),
        (
            "---@input o choice(a, b",
            "`choice(` needs its closing `)`",
            "choice(a, b",
        ),
        (
            "---@input o choice(a, )",
            "A choice lists its words",
            "choice(a, )",
        ),
        ("---@input c color = red", "“red” is not a colour", "red"),
        (
            "---@input s string = x -- Name {1..2}",
            "Only a number or an integer takes a range",
            " s string = x -- Name {1..2}",
        ),
        (
            "---@input n number = 5 -- N {12..1}",
            "A range reads `{lo..hi}`, low first",
            " n number = 5 -- N {12..1}",
        ),
        ("---@input n number 5", "A default follows `=`", " 5"),
        ("---@input n", "“n” needs a type", " n"),
    ];
    for (source, message, underlined) in cases {
        let err = header(source).unwrap_err();
        assert!(
            err.message.starts_with(message),
            "{source}: {}",
            err.message
        );
        let span = err.span.clone().unwrap();
        assert_eq!(&source[span], underlined, "{message}");
    }
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

/// A set stop flag ends a run as stopped: at the next step of a loop, and
/// between the per-layer host calls of one write, where no interrupt fires.
/// flip: `State::on_time` not reading the flag → fails at "one layer written".
#[test]
fn a_stop_flag_ends_the_run() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let leash = |stop: &Arc<AtomicBool>| Leash {
        budget: WORKER_BUDGET,
        stop: Some(stop.clone()),
    };
    let stop = Arc::new(AtomicBool::new(true));
    let err = run_script_leashed(&mut h, &e, "while true do end", &leash(&stop)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stopped);
    assert_eq!(err.message, "Stopped");
    // Stopped by the first verb: the write's other two layers never run.
    let stop = Arc::new(AtomicBool::new(false));
    h.stop_on_verb = Some(stop.clone());
    let before = h.verbs;
    let err = run_script_leashed(
        &mut h,
        &e,
        "while true do sel.name = \"x\" end",
        &leash(&stop),
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Stopped);
    assert_eq!(h.verbs - before, 1, "one layer written");
}

/// The clock is read at once after a verb, so a loop of slow verbs stops near
/// the time budget rather than at the next 256th step.
/// flip: the interrupt not reading the clock after a host call → fails at the
/// count of verbs called.
#[test]
fn a_slow_verb_is_timed_at_once() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids[..1], &labels);
    h.slow = std::time::Duration::from_millis(30);
    let before = h.verbs;
    let err = run_script(&mut h, &e, "for i = 1, 200 do set_name(\"x\") end").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Budget);
    let called = h.verbs - before;
    assert!(called < 45, "{called} verbs ran past the 1 s budget");
}

/// The sandbox's walls: no name off the allowlist resolves, the memory budget
/// stops a run that allocates past it, and the interrupt stops a runaway inside
/// a function Luau's own library calls back. A comment gives each part's flip.
#[test]
fn the_sandbox_holds() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    // flip: `resolve` falling back to `lua.globals()` → fails at "getfenv".
    for name in [
        "getfenv",
        "setfenv",
        "load",
        "loadstring",
        "pcall",
        "xpcall",
        "rawset",
        "rawget",
        "rawequal",
        "setmetatable",
        "getmetatable",
        "_G",
        "_VERSION",
        "coroutine",
        "debug",
        "utf8",
        "buffer",
        "bit32",
        "os",
        "require",
        "newproxy",
        "gcinfo",
        "collectgarbage",
    ] {
        let err = run_script(&mut h, &e, &format!("return {name}"))
            .map(|o| o.value)
            .expect_err(name);
        assert!(
            err.message.starts_with(&format!("No name “{name}”")),
            "{name}: {}",
            err.message
        );
    }
    // flip: `set_memory_limit` dropped → fails at "string.rep".
    for (what, src) in [
        ("string.rep", "local s = string.rep('x', 1e8) return #s"),
        ("table.create", "local t = table.create(1e7, 0) return #t"),
    ] {
        let err = run_script(&mut h, &e, src)
            .map(|o| o.value)
            .expect_err(what);
        assert_eq!(err.kind, ErrorKind::Budget, "{what}: {}", err.message);
        assert_eq!(
            err.message, "Stopped: the run used more than 64 MB",
            "{what}"
        );
    }
    // No edit of this crate breaks only these two: Luau calls the interrupt in
    // every frame. They hold that for the next mlua or Luau bump.
    let err = eval_read(
        &mut h,
        &e,
        "(function() local t = {3, 2, 1} table.sort(t, function(a, b) while true do end end) end)()",
    )
    .expect_err("a sort's comparator");
    assert_eq!(err.kind, ErrorKind::Budget, "{}", err.message);
    let err = run_script(
        &mut h,
        &e,
        "sel:adjust(\"x\", function(x) while true do end end)",
    )
    .map(|o| o.value)
    .expect_err("an adjust callback");
    assert_eq!(err.kind, ErrorKind::Budget, "{}", err.message);
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

/// The lexer ends strings, long brackets and comments where Luau does, each case
/// held to a run: a script whose `sel.x = 5` the lexer reads as code moves the
/// layer, and one it reads as text does not.
/// flip: `escape_end` taking two bytes for `\z` → fails at the `\z` case.
#[test]
fn the_lexer_reads_strings_and_brackets_as_luau_does() {
    let cases: [(&str, bool); 13] = [
        ("local s = \"a\\z\n\" sel.x = 5 --\"", true),
        ("local s = \"a\\z  \n\t \" sel.x = 5 --\"", true),
        ("local s = \"a\\\r\n\" sel.x = 5 --\"", true),
        ("local s = `a\\z\n` sel.x = 5 --`", true),
        ("local s = \"a\\\"sel.x = 5\"", false),
        ("local s = \"a\\\nsel.x = 5\"", false),
        ("local s = [==[ ]] sel.x = 5 ]==]", false),
        ("local s = [==[ ]] ]==] sel.x = 5", true),
        ("--[[ sel.x = 5", false),
        ("--[==[ ]] sel.x = 5 ]==]", false),
        ("--[= note\nsel.x = 5", true),
        ("local s = `a {`b {sel.x}`} c` sel.x = 5", true),
        ("local s = [[ sel.x = 5", false),
    ];
    let labels = FxHashMap::default();
    for (src, runs) in cases {
        let used = names_used(src);
        let expected: &[&str] = if runs { &["sel", "x"] } else { &[] };
        assert_eq!(used, expected, "{src:?}");
        let (mut h, ids) = three();
        let ran = run_script(&mut h, &env(&ids[..1], &labels), src).is_ok() && h.x(ids[0]) == 5.0;
        assert_eq!(ran, runs, "Luau agrees: {src:?}");
    }
    let src = "print(1.5, 1e-2, 0x1e-2, 0x10)";
    let numbers: Vec<&str> = lex::lex(src)
        .into_iter()
        .filter(|l| l.kind == lex::Lx::Number)
        .map(|l| &src[l.range])
        .collect();
    assert_eq!(
        numbers,
        ["1.5", "1e-2", "0x1e", "2", "0x10"],
        "a hex `e` is a digit"
    );
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

/// `print` builds its line against the room left under `PRINT_CAP`, so many long
/// arguments stop the run before a line past the cap is built.
/// flip: the `print` callback building every argument before the cap is checked
/// → fails at the longest line built.
#[test]
fn print_stops_before_building_past_the_cap() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    crate::run::PRINT_BUILT.set(0);
    let src = "local s = string.rep('x', 1e5) print(table.unpack(table.create(100, s)))";
    let err = run_script(&mut h, &e, src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Budget);
    assert_eq!(
        err.message,
        "Stopped after printing more than 10,000 lines or 1 MB"
    );
    let built = crate::run::PRINT_BUILT.get();
    assert!(built > 0, "the probe saw the line");
    assert!(built <= 1 << 20, "a line of {built} bytes was built");
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
    // An input the header does not declare, and a property through `local c = sel`.
    // flip: `check` skipping the `param` arm → fails at "colz".
    let src = "---@input cols integer = 3\nprint(param.colz)\nlocal c = sel\nc.widht = 1";
    let found: Vec<(u32, String)> = check(&h.doc, h.page, src)
        .into_iter()
        .map(|p| (p.line, p.message))
        .collect();
    assert_eq!(
        found,
        [
            (2, "No input “colz” — did you mean cols?".to_owned()),
            (4, "No property “widht” — did you mean width?".to_owned()),
        ]
    );
}

/// A type alias and a generic function's type parameters are types, and its
/// parameters are declared: the check marks nothing in either.
/// flip: `declarations` not skipping a generic `<…>` → fails at "generic".
#[test]
fn type_aliases_and_generics_are_known() {
    let (h, _) = three();
    let src = "type Point = { x: number, y: number }\n\
               export type Pair<T> = { a: T, b: T }\n\
               local function id<T>(v: T): T return v end\n\
               local p: Point = { x = 1, y = 2 }\nprint(id(p))";
    let found: Vec<String> = check(&h.doc, h.page, src)
        .into_iter()
        .map(|p| p.message)
        .collect();
    assert_eq!(found, Vec::<String>::new(), "generic: nothing marked");
    let marked = |src: &str| -> Vec<String> {
        check(&h.doc, h.page, src)
            .into_iter()
            .map(|p| p.message)
            .collect()
    };
    assert_eq!(
        marked("function f<T>(v: T) return v end"),
        marked("function f(v) return v end"),
        "a generic global function reads as a plain one"
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

/// Paints, effects, text style and layout read back what was written, and
/// writing back what was read leaves the value as it was. A text in its style
/// colour reads it as its fill, and fills written to it take effect.
/// flip: `props::short` skipping `verbs::shorten` → fails at "reads as written"
/// for the fills' hex; `write_model` clearing the style colour only where the
/// stored fills changed → fails at "fills equal to the stored ones".
#[test]
fn modelled_properties_round_trip() {
    let mut h = TestHost::new();
    let frame = h.frame("Card", 400.0, 300.0);
    let r = h.rect(frame, "Box", 0.0, 0.0, 40.0, 40.0);
    let t = h.make(
        "create_text",
        json!({ "parent": frame.to_wire(), "text": "Hello" }),
    )[0];
    let labels = FxHashMap::default();
    let run = |h: &mut TestHost, id: NodeId, src: &str| {
        run_script(h, &env(&[id], &labels), src).unwrap_or_else(|e| panic!("{src}: {}", e.message))
    };
    let read = |h: &TestHost, id: NodeId, prop: &str| crate::props::read(&h.doc, &h.res, id, prop);
    // A gradient under a plain colour, a shadow, a dashed stroke, a flex layout.
    use mastik_core::peniko::{Color, Gradient};
    let gradient = mastik_core::Fill {
        brush: mastik_core::Brush::Gradient(
            Gradient::new_linear((0.0, 0.0), (40.0, 40.0))
                .with_stops([
                    Color::from_rgb8(0x12, 0x35, 0x2a),
                    Color::from_rgb8(0xf0, 0xc0, 0x40),
                ])
                .into(),
        )
        .into(),
        visible: false,
    };
    let fills = json!(["#ff0000", serde_json::to_value(&gradient).unwrap()]);
    for (verb, key, model) in [
        ("set_fills", "fills", "[Fill]"),
        ("set_strokes", "strokes", "[Stroke]"),
        ("set_effects", "effects", "[Effect]"),
    ] {
        let value = if key == "fills" {
            fills.clone()
        } else {
            verbs::example(model).unwrap()
        };
        h.make(verb, json!({ "nodes": [r.to_wire()], key: value }));
    }
    h.make(
        "set_display",
        json!({ "nodes": [frame.to_wire()], "display": verbs::example("Display").unwrap() }),
    );
    h.make(
        "set_layout_item",
        json!({ "nodes": [r.to_wire()], "item": verbs::example("LayoutItem").unwrap() }),
    );
    for (id, prop, least) in [(r, "fills", 2), (r, "strokes", 1), (r, "effects", 1)] {
        let v = read(&h, id, prop).unwrap();
        assert!(
            matches!(&v, crate::props::Val::Json(Value::Array(a)) if a.len() >= least && a.last().is_some_and(Value::is_object)),
            "the fixture's {prop} hold a table: {v:?}"
        );
    }
    assert!(matches!(
        read(&h, frame, "layout").unwrap(),
        crate::props::Val::Json(_)
    ));
    let cases: &[(NodeId, &str)] = &[
        (r, "fills"),
        (r, "strokes"),
        (r, "effects"),
        (r, "layout_item"),
        (frame, "layout"),
        (frame, "gap"),
        (frame, "padding"),
        (frame, "direction"),
        (t, "font_family"),
        (t, "font_size"),
        (t, "font_weight"),
        (t, "italic"),
        (t, "line_height"),
        (t, "letter_spacing"),
        (t, "text_case"),
        (t, "text_align"),
    ];
    for (id, prop) in cases {
        let before = read(&h, *id, prop).unwrap();
        assert!(
            !matches!(before, crate::props::Val::Mixed(_)),
            "{prop} reads one value: {before:?}"
        );
        run(
            &mut h,
            *id,
            &format!("local v = sel.{prop}\nsel.{prop} = v"),
        );
        assert_eq!(read(&h, *id, prop).unwrap(), before, "{prop} as it was");
    }
    // Each reads as written.
    let fills_read = run(&mut h, r, "return sel.fills[1]").copied;
    assert_eq!(fills_read.as_deref(), Some("#ff0000"), "reads as written");
    // One gradient is `.fill`'s table, and writes back as itself.
    run(&mut h, r, "sel.fills = { sel.fills[2] }");
    let kind = run(&mut h, r, "return type(sel.fill)").copied;
    assert_eq!(
        kind.as_deref(),
        Some("table"),
        "a gradient reads as its table"
    );
    let before = read(&h, r, "fills").unwrap();
    run(
        &mut h,
        r,
        "local f = sel.fill\nsel.fill = nil\nsel.fill = f",
    );
    assert_eq!(read(&h, r, "fills").unwrap(), before, "fill as it was");
    // A text drawing in its style colour reads it as its fill, so its fills
    // written back draw as they did, and any fill written takes effect, its
    // stored black one too.
    let red = h.make(
        "create_text",
        json!({ "parent": frame.to_wire(), "text": "Red" }),
    )[0];
    h.make(
        "set_fills",
        json!({ "nodes": [red.to_wire()], "fills": ["#000000"] }),
    );
    h.make(
        "set_text_style",
        json!({ "nodes": [red.to_wire()], "style": { "color": "#ff0000" } }),
    );
    let coloured = |h: &TestHost| matches!(h.doc.get(red).unwrap().kind(), NodeKind::Text { style, .. } if style.color.is_some());
    assert!(coloured(&h), "fixture: the text has a style colour");
    let stored = |h: &TestHost| {
        h.doc.get(red).unwrap().paint().fills[0]
            .value
            .brush
            .literal()
            .cloned()
    };
    assert_eq!(
        stored(&h),
        Some(mastik_core::Brush::Solid(Color::from_rgb8(0, 0, 0))),
        "fixture: its stored fill is black"
    );
    let drawn = crate::props::Val::Str("#ff0000".into());
    assert_eq!(read(&h, red, "fill").unwrap(), drawn, "reads as drawn");
    run(&mut h, red, "sel.fills = { \"#000000\" }");
    assert!(
        !coloured(&h),
        "fills equal to the stored ones clear the style colour"
    );
    assert_eq!(
        read(&h, red, "fill").unwrap(),
        crate::props::Val::Str("#000000".into()),
        "black"
    );
    h.make(
        "set_text_style",
        json!({ "nodes": [red.to_wire()], "style": { "color": "#ff0000" } }),
    );
    run(&mut h, red, "local f = sel.fills\nsel.fills = f");
    assert_eq!(
        read(&h, red, "fill").unwrap(),
        drawn,
        "written back, as drawn"
    );
    for (id, src, prop, want) in [
        (t, "sel.font_size = 24", "font_size", "24"),
        (t, "sel.font_weight = 700", "font_weight", "700"),
        (t, "sel.italic = true", "italic", "true"),
        (t, "sel.line_height = 30", "line_height", "30"),
        (t, "sel.line_height = \"1.5em\"", "line_height", "\"1.5em\""),
        (t, "sel.line_height = nil", "line_height", "nil"),
        (
            t,
            "sel.letter_spacing = \"5%\"",
            "letter_spacing",
            "\"0.05em\"",
        ),
        (t, "sel.text_case = \"upper\"", "text_case", "\"Upper\""),
        (t, "sel.text_align = \"center\"", "text_align", "\"Center\""),
        (t, "sel.font_family = \"Inter\"", "font_family", "\"Inter\""),
        (frame, "sel.gap = 18", "gap", "18"),
        (frame, "sel.padding = 8", "padding", "8"),
        (
            frame,
            "sel.padding = {1, 2, 3, 4}",
            "padding",
            "{1, 2, 3, 4}",
        ),
        (
            frame,
            "sel.direction = \"column\"",
            "direction",
            "\"Column\"",
        ),
        (
            frame,
            "sel.direction = \"RowReverse\"",
            "direction",
            "\"RowReverse\"",
        ),
        (
            frame,
            "local d = sel.direction\nsel.direction = d",
            "direction",
            "\"RowReverse\"",
        ),
        (
            frame,
            "sel.direction = \"column-reverse\"",
            "direction",
            "\"ColumnReverse\"",
        ),
        (r, "sel.fill = \"#00ff00\"", "fill", "\"#00ff00\""),
        (r, "sel.effects = {}", "effects", "{}"),
        (frame, "sel.layout = nil", "layout", "nil"),
    ] {
        run(&mut h, id, src);
        let got = run(&mut h, id, &format!("return sel.{prop}"))
            .value
            .unwrap_or_else(|| "nil".into());
        assert_eq!(got, want, "{src}");
    }
    // An empty list read from the model writes back as a list, never an object.
    // flip: `is_list` answering false → fails here, "expected a sequence".
    run(
        &mut h,
        r,
        "sel.strokes = { { brush = \"#000000\", width = 2, dashes = sel.effects } }",
    );
    assert_eq!(
        run(&mut h, r, "return sel.strokes[1].width")
            .value
            .as_deref(),
        Some("2")
    );
    // `adjust` steps a text's size, and a text's property refuses a frame.
    run(&mut h, t, "sel:adjust(\"font_size\", 2)");
    assert_eq!(
        read(&h, t, "font_size").unwrap(),
        crate::props::Val::Num(26.0)
    );
    let err = run_script(&mut h, &env(&[r], &labels), "sel.font_size = 12").unwrap_err();
    assert!(
        err.message.contains("“Box” is not a text"),
        "{}",
        err.message
    );
    let err = run_script(&mut h, &env(&[frame], &labels), "sel.gap = 4").unwrap_err();
    assert!(
        err.message.contains("set its .layout first"),
        "{}",
        err.message
    );
}

/// `set_by` names what a property edit set, as the line that makes it: run on a
/// twin layer, that line makes the same edit.
///
/// flip: `set_by`'s `SetDisplay` arm skipping `layout_parts` → fails at the
/// names `sel.gap = 20` set (the whole `.layout` comes back).
#[test]
fn set_by_names_an_edit_as_the_line_that_repeats_it() {
    let mut h = TestHost::new();
    let frames = [h.frame("A", 400.0, 300.0), h.frame("B", 400.0, 300.0)];
    let rects = frames.map(|f| h.rect(f, "Box", 0.0, 0.0, 40.0, 40.0));
    let texts = frames.map(|f| {
        h.make(
            "create_text",
            json!({ "parent": f.to_wire(), "text": "Hi" }),
        )[0]
    });
    // "Hello world" with "world" bold: an attribute the edit leaves alone differs
    // across the runs.
    let bold = frames.map(|f| {
        let t = h.make(
            "create_text",
            json!({ "parent": f.to_wire(), "text": "Hello world" }),
        )[0];
        h.make(
            "style_text",
            json!({ "node": t.to_wire(), "attrs": [{ "Weight": 700 }], "start": 6 }),
        );
        t
    });
    let labels = FxHashMap::default();
    let display = crate::props::json_literal(&verbs::example("Display").unwrap());
    let effects = crate::props::json_literal(&verbs::example("[Effect]").unwrap());
    let steps: Vec<([NodeId; 2], String, &[&str])> = vec![
        (bold, "sel.font_size = 24".into(), &["font_size"]),
        (rects, "sel.fill = \"#336699\"".into(), &["fill"]),
        (
            rects,
            "sel.fills = { \"#ff0000\", \"#00ff00\" }".into(),
            &["fills"],
        ),
        (rects, format!("sel.effects = {effects}"), &["effects"]),
        (texts, "sel.font_size = 24".into(), &["font_size"]),
        (texts, "sel.font_weight = 700".into(), &["font_weight"]),
        (texts, "sel.text_align = \"Center\"".into(), &["text_align"]),
        (frames, format!("sel.layout = {display}"), &["layout"]),
        (frames, "sel.gap = 20".into(), &["gap"]),
        (frames, "sel.padding = 8".into(), &["padding"]),
        (frames, "sel.direction = \"Column\"".into(), &["direction"]),
        (
            frames,
            "sel.direction = \"RowReverse\"".into(),
            &["direction"],
        ),
    ];
    for ([a, b], line, props) in steps {
        run_script(&mut h, &env(&[a], &labels), &line)
            .unwrap_or_else(|e| panic!("{line}: {}", e.message));
        let inverse = h.last.take().expect("fixture: the line committed");
        let mut set = Vec::new();
        for op in &inverse.0 {
            set.extend(
                crate::set_by(&h.doc, &h.res, op).unwrap_or_else(|| panic!("{line}: {op:?}")),
            );
        }
        let named: Vec<&str> = set.iter().map(|(p, _)| *p).collect();
        assert_eq!(named, props, "{line}: set {named:?}");
        let again: String = set
            .iter()
            .map(|(p, v)| format!("sel.{p} = {v}\n"))
            .collect();
        run_script(&mut h, &env(&[b], &labels), &again)
            .unwrap_or_else(|e| panic!("{again}: {}", e.message));
        for (prop, _) in &set {
            assert_eq!(
                crate::props::read(&h.doc, &h.res, a, prop).unwrap(),
                crate::props::read(&h.doc, &h.res, b, prop).unwrap(),
                "{again}: the twin's {prop}"
            );
        }
    }
}

/// A bound text size, gap, padding and opacity read at their variables' values,
/// never the 0 a bound slot's literal holds.
/// flip: `read`'s text arms reading the node's own kind → fails at "font size".
#[test]
fn a_bound_value_reads_as_drawn() {
    let mut h = TestHost::new();
    let frame = h.frame("Card", 400.0, 300.0);
    let t = h.make(
        "create_text",
        json!({ "parent": frame.to_wire(), "text": "Hello" }),
    )[0];
    h.make(
        "set_display",
        json!({ "nodes": [frame.to_wire()], "display": verbs::example("Display").unwrap() }),
    );
    let labels = FxHashMap::default();
    let bind = |h: &mut TestHost, id: NodeId, src: &str| {
        run_script(h, &env(&[id], &labels), src).unwrap_or_else(|e| panic!("{src}: {}", e.message));
    };
    bind(
        &mut h,
        t,
        "local v = add_variable{kind = \"number\", name = \"type/size\", value = {number = 30}}\n\
         bind_variable{nodes = sel, field = {type = 2}, variable = v}\n\
         local o = add_variable{kind = \"number\", name = \"fade\", value = {number = 50}}\n\
         bind_variable{nodes = sel, field = \"opacity\", variable = o}",
    );
    bind(
        &mut h,
        frame,
        "local v = add_variable{kind = \"number\", name = \"space/24\", value = {number = 24}}\n\
         bind_variable{nodes = sel, field = {spacing = 63}, variable = v}",
    );
    let node = |id: NodeId| h.doc.get(id).unwrap();
    assert!(
        node(t).text_vars()[mastik_core::variable::TEXT_SIZE].is_some()
            && node(frame).spacing_vars().iter().all(Option::is_some),
        "fixture: the slots are bound"
    );
    let read = |id: NodeId, prop: &str| crate::props::read(&h.doc, &h.res, id, prop).unwrap();
    use crate::props::Val;
    assert_eq!(read(t, "font_size"), Val::Num(30.0), "font size");
    assert_eq!(read(t, "opacity"), Val::Num(0.5), "opacity");
    assert_eq!(read(frame, "gap"), Val::Num(24.0), "gap");
    assert_eq!(read(frame, "padding"), Val::Num(24.0), "padding");
}

/// A hugging main with a width query reads its item as authored, not the
/// padding width layout contains it to, so the item written back with a change
/// keeps hugging.
/// flip: the `layout_item` arm reading `resolved_item` → fails at "still hugs".
#[test]
fn a_query_frame_reads_its_item_as_authored() {
    use mastik_core::container::Dimension;
    let mut h = TestHost::new();
    let card = h.frame("Card", 300.0, 200.0);
    h.make(
        "set_display",
        json!({ "nodes": [card.to_wire()], "display": verbs::example("Display").unwrap() }),
    );
    h.make("make_component", json!({ "nodes": [card.to_wire()] }));
    for name in ["Narrow", "Wide"] {
        h.make("add_mode", json!({ "name": name }));
    }
    h.make(
        "set_width_query",
        json!({ "node": card.to_wire(), "on": true }),
    );
    h.make(
        "set_layout_item",
        json!({ "nodes": [card.to_wire()], "item": { "width": "FitContent" } }),
    );
    let item = |h: &TestHost| *h.doc.get(card).unwrap().item();
    assert!(
        !h.doc.get(card).unwrap().queries().is_empty() && item(&h).width == Dimension::FitContent,
        "fixture: a hugging main with a width query"
    );
    assert_ne!(
        h.doc.resolved_item(card).width,
        Dimension::FitContent,
        "fixture: laid out at its padding"
    );
    let labels = FxHashMap::default();
    run_script(
        &mut h,
        &env(&[card], &labels),
        "local i = sel.layout_item\ni.grow = 1\nsel.layout_item = i",
    )
    .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(item(&h).width, Dimension::FitContent, "still hugs");
    assert_eq!(item(&h).grow, 1.0, "the change landed");
}

/// A text edit that also changes a field no property names has no line, so `.`
/// cannot repeat half of it: the Type panel's family pick, which clears the
/// features with it.
/// flip: `char_changes` skipping the kinds `CHAR_PROPS` leaves out → fails at
/// "a hole".
#[test]
fn set_by_leaves_a_hole_for_a_field_no_property_names() {
    let mut h = TestHost::new();
    let frame = h.frame("A", 400.0, 300.0);
    let t = h.make(
        "create_text",
        json!({ "parent": frame.to_wire(), "text": "Hi" }),
    )[0];
    h.make(
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "features": [{ "tag": "ss01", "value": 1 }] } }),
    );
    h.make(
        "set_text_style",
        json!({ "nodes": [t.to_wire()], "style": { "font_family": "Georgia", "features": [] } }),
    );
    let NodeKind::Text { style, .. } = h.doc.get(t).unwrap().kind() else {
        panic!("a text");
    };
    assert!(
        style.features.is_empty() && style.font_family == "Georgia",
        "fixture: the pick landed"
    );
    let inverse = h.last.take().unwrap();
    for op in &inverse.0 {
        assert_eq!(crate::set_by(&h.doc, &h.res, op), None, "a hole: {op:?}");
    }
}

/// `random(seed)` draws the same numbers for the same seed, in each of its
/// three forms and within their bounds, and a seed is required.
/// flip: `Rng::new` ignoring its seed → fails at "another seed draws others".
#[test]
fn random_draws_are_fixed_by_their_seed() {
    let (mut h, ids) = three();
    let labels = FxHashMap::default();
    let e = env(&ids, &labels);
    let draws = "local r = random(SEED)\nlocal out = {}\n\
                 for i = 1, 200 do table.insert(out, r()) end\n\
                 for i = 1, 200 do table.insert(out, r(6)) end\n\
                 for i = 1, 200 do table.insert(out, r(-3, 3)) end\n\
                 return table.concat(out, ',')";
    let run = |h: &mut TestHost, seed: &str| {
        run_script(h, &e, &draws.replace("SEED", seed))
            .unwrap()
            .copied
            .unwrap()
    };
    let first = run(&mut h, "42");
    // `assert!` rather than `assert_eq!`: the draws are 600 numbers long.
    assert!(first == run(&mut h, "42"), "the same seed draws the same");
    assert!(first == run(&mut h, "42.0"), "a whole seed is itself");
    assert!(first != run(&mut h, "43"), "another seed draws others");
    assert!(run(&mut h, "\"grid\"") == run(&mut h, "\"grid\""));
    let values: Vec<f64> = first.split(',').map(|v| v.parse().unwrap()).collect();
    assert!(values[..200].iter().all(|v| (0.0..1.0).contains(v)));
    assert!(
        values[200..400]
            .iter()
            .all(|v| (1.0..=6.0).contains(v) && v.fract() == 0.0)
    );
    assert!(
        values[400..]
            .iter()
            .all(|v| (-3.0..=3.0).contains(v) && v.fract() == 0.0)
    );
    for n in 1..=6 {
        assert!(values[200..400].contains(&f64::from(n)), "{n} is drawn");
    }
    // The generator is the crate's own, so the numbers a seed draws never move.
    let out = run_script(&mut h, &e, "local r = random(1) return r(1000000)").unwrap();
    assert_eq!(out.value.as_deref(), Some("702922"), "pinned");
    // One pin for each way to a seed (a string's hash, a fraction's bits, a
    // negative whole, a whole too big for its own value) and for the fraction.
    for (src, want) in [
        ("random(\"grid\")(1000000)", "293241"),
        ("random(0.5)(1000000)", "680323"),
        ("random(-1)(1000000)", "559893"),
        ("random(1e16)(1000000)", "313054"),
        ("random(1)() * 2^53", "6331357011769570"),
    ] {
        let out = run_script(&mut h, &e, &format!("return {src}")).unwrap();
        assert_eq!(out.value.as_deref(), Some(want), "{src} pinned");
    }
    for (src, said) in [
        ("random(0/0)", "`random`'s seed is a number or a string"),
        (
            "random(math.huge)",
            "`random`'s seed is a number or a string",
        ),
        ("random()", "`random` needs a seed"),
        ("random({})", "`random`'s seed is a number or a string"),
        ("random(1)(2.5)", "A random draw takes whole numbers"),
        (
            "random(1)(3, 1)",
            "A random draw from 3 to 1 has no whole number",
        ),
    ] {
        let err = run_script(&mut h, &e, src).unwrap_err();
        assert!(err.message.starts_with(said), "{src}: {}", err.message);
    }
    let out = run_script(&mut h, &e, "return tostring(random(7))").unwrap();
    assert_eq!(out.copied.as_deref(), Some("random(7)"));
}

/// A host with table People: Name and Photo, three rows.
fn people() -> TestHost {
    let mut h = TestHost::new();
    h.make(
        "add_table",
        json!({
            "name": "People",
            "columns": ["Name", "Photo"],
            "rows": [["Ada", ""], ["Grace", "grace.png"], ["Alan", ""]],
        }),
    );
    h
}

/// The cells of table `name` in `h`'s document.
fn cells(h: &TestHost, name: &str) -> Vec<Vec<String>> {
    verbs::find_table(&h.doc, name).unwrap().table.rows.clone()
}

/// `tables` reads cells and counts, and a run's writes land at its end as one
/// commit, a read after a write seeing it.
/// flip: `flush` not called at the run's end → fails at "one edit for the run".
#[test]
fn tables_reads_and_gathers_writes_into_one_edit() {
    let mut h = people();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    let out = run_script(&mut h, &e, "return tables.People[2].Name").unwrap();
    assert_eq!(out.copied.as_deref(), Some("Grace"));
    let before = h.commits;
    let src = "local p = tables.People\n\
               for i = 1, p.rows do\n\
                 if p[i].Photo == \"\" then p:set(i, { Photo = \"placeholder.png\" }) end\n\
               end\n\
               p:add({ Name = \"Mae\", Photo = 7 })\n\
               p:remove(2)\n\
               return #p .. \" \" .. p[3].Name";
    let out = run_script(&mut h, &e, src).unwrap();
    assert_eq!(out.copied.as_deref(), Some("3 Mae"), "reads see the writes");
    assert_eq!(h.commits, before + 1, "one edit for the run");
    assert_eq!(
        cells(&h, "People"),
        [
            ["Ada", "placeholder.png"],
            ["Alan", "placeholder.png"],
            ["Mae", "7"],
        ],
        "written back"
    );
}

/// Table names complete after `tables.` in the bar and a script, and for a
/// verb's text argument naming a table (`table`, `set_chart_data`'s `data`),
/// each with its size; a chart's `table` model takes no name.
/// flip: `verb_argument` not offering `table_names` → fails at "the verb's
/// argument"; offering them for any param named `table` → fails at "a chart's
/// own table".
#[test]
fn table_names_complete_after_tables_and_for_a_table_argument() {
    let h = people();
    let labels = FxHashMap::default();
    let label = |c: Completion| c.rows.first().map(|r| (r.label.clone(), r.detail.clone()));
    let want = Some(("People".to_owned(), "3 rows \u{b7} 2 columns".to_owned()));
    let c = complete(&h.doc, h.page, &labels, "edit_table Pe", 13);
    assert_eq!(label(c), want, "the verb's argument");
    let c = complete(&h.doc, h.page, &labels, "set_chart_data Pe", 17);
    assert_eq!(label(c), None, "a chart's own table");
    let c = complete(&h.doc, h.page, &labels, "set_chart_data x Pe", 19);
    assert_eq!(label(c), want, "set_chart_data's data");
    let c = complete(&h.doc, h.page, &labels, "print(tables.Pe", 15);
    assert_eq!(label(c), want, "after tables. in the bar");
    let c = complete_script(&h.doc, h.page, "local t = tables.P", 18);
    assert_eq!(label(c), want, "after tables. in a script");
}

/// A verb's variable and script arguments complete with their names and a
/// note each, and the name completion writes runs on the one it names.
/// flip: `verb_argument` not offering `id_names` → fails at "a variable
/// argument".
#[test]
fn id_arguments_complete_by_name_and_run_on_what_they_name() {
    let mut h = TestHost::new();
    h.make("add_variable", json!({ "kind": "number", "name": "Count" }));
    h.make(
        "add_script",
        json!({ "name": "Tidy", "source": "print(1)\nprint(2)" }),
    );
    let labels = FxHashMap::default();
    let label = |c: Completion| c.rows.first().map(|r| (r.label.clone(), r.detail.clone()));
    let c = complete(&h.doc, h.page, &labels, "rename_variable Co", 18);
    assert_eq!(
        label(c),
        Some(("Count".to_owned(), "number \u{b7} Variables".to_owned())),
        "a variable argument"
    );
    let c = complete(&h.doc, h.page, &labels, "remove_script Ti", 16);
    assert_eq!(
        label(c),
        Some(("Tidy".to_owned(), "2 lines".to_owned())),
        "a script argument"
    );
    let e = env(&[], &labels);
    run_line(&mut h, &e, "rename_variable Count Total").unwrap();
    assert_eq!(
        h.doc.variables().ids_named("Total").len(),
        1,
        "renamed by its name"
    );
    let err = run_line(&mut h, &e, "remove_script Nothing").unwrap_err();
    assert!(err.message.contains("script"), "{}", err.message);
}

/// A layer's reactions and play settings and its page's flows read in the
/// shapes their verbs take, so a script copies them from one layer to others.
/// flip: `props::read` answering nil for `reactions` → fails at "copied to
/// both".
#[test]
fn prototype_properties_read_as_their_verbs_take_them() {
    let mut h = TestHost::new();
    let a = h.frame("A", 100.0, 100.0);
    let b = h.frame("B", 100.0, 100.0);
    let one = h.rect(a, "One", 0.0, 0.0, 10.0, 10.0);
    let two = h.rect(a, "Two", 20.0, 0.0, 10.0, 10.0);
    let three = h.rect(a, "Three", 40.0, 0.0, 10.0, 10.0);
    h.make(
        "set_reactions",
        json!({ "nodes": [one.to_wire()], "reactions": [
            { "trigger": "click", "actions": [{ "navigate": { "to": b.to_wire() } }] },
        ] }),
    );
    h.make(
        "set_play",
        json!({ "nodes": [a.to_wire()], "play": { "scroll": "Vertical" } }),
    );
    let labels = FxHashMap::default();
    let selected = [two, three];
    let e = env(&selected, &labels);
    run_script(
        &mut h,
        &e,
        "sel.reactions = One.reactions\nB.play = A.play\nA.flows = { { name = \"Main\", start = A.id } }",
    )
    .unwrap();
    let held = |h: &TestHost, id: NodeId| h.doc.get(id).unwrap().reactions().len();
    assert_eq!((held(&h, two), held(&h, three)), (1, 1), "copied to both");
    assert_eq!(
        h.doc.get(two).unwrap().reactions()[0].value.actions[0].value,
        h.doc.get(one).unwrap().reactions()[0].value.actions[0].value,
        "the same action"
    );
    assert_eq!(
        h.doc.get(b).unwrap().play(),
        h.doc.get(a).unwrap().play(),
        "the settings copied"
    );
    let out = run_script(&mut h, &e, "return One.flows[1].name").unwrap();
    assert_eq!(out.copied.as_deref(), Some("Main"), "the page's flows");
    let out = run_script(&mut h, &e, "return #Two.reactions[1].actions").unwrap();
    assert_eq!(out.copied.as_deref(), Some("1"));
}

/// A run that fails part-way writes no table, and a write names what is wrong.
#[test]
fn tables_refuses_and_a_failed_run_writes_nothing() {
    let mut h = people();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    let src = "tables.People:set(1, { Name = \"X\" })\nerror(\"stop\")";
    assert!(run_script(&mut h, &e, src).is_err());
    assert_eq!(cells(&h, "People")[0][0], "Ada", "nothing written");
    let err = run_script(&mut h, &e, "tables.People:set(9, { Name = \"X\" })").unwrap_err();
    assert_eq!(err.message, "People has 3 rows, not 9");
    let err = run_script(&mut h, &e, "tables.People:add({ Age = 3 })").unwrap_err();
    assert_eq!(err.message, "People has no column “Age”");
    let err = run_script(&mut h, &e, "return tables.Nobody.rows").unwrap_err();
    assert!(
        err.message.starts_with("No table “Nobody”"),
        "{}",
        err.message
    );
    let src =
        "local t = tables.new(\"Cart\", {\"Name\"})\nt:add({ Name = \"Shirt\" })\nreturn t.name";
    let out = run_script(&mut h, &e, src).unwrap();
    assert_eq!(out.copied.as_deref(), Some("Cart"));
    assert_eq!(cells(&h, "Cart"), [["Shirt"]]);
}

/// A write to a table linked to a file is refused at its line, naming the
/// file, and writes nothing.
/// flip: `editable` not judging the set's `source` → fails at "refused at its
/// line".
#[test]
fn a_write_to_a_linked_table_is_refused_at_its_line() {
    let mut h = people();
    let mut set = verbs::find_table(&h.doc, "People").unwrap().clone();
    set.source = Some(mastik_core::chart::DataLink {
        path: "C:/data/people.csv".into(),
        size: 0,
        modified: 0,
        header: None,
    });
    h.doc
        .apply(&Transaction(vec![mastik_core::Operation::SetDataSet {
            id: set.id,
            set: Some(Box::new(set)),
        }]))
        .unwrap();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    let src = "local n = 1\ntables.People:set(1, { Name = \"X\" })";
    let err = run_script(&mut h, &e, src).unwrap_err();
    assert_eq!(
        (err.line, err.message.as_str()),
        (Some(2), "People is linked to people.csv: unlink it first"),
        "refused at its line"
    );
    assert_eq!(cells(&h, "People")[0][0], "Ada", "nothing written");
}

/// A number a script writes to a cell is written as Luau prints it, every
/// digit kept.
/// flip: `cell_text` writing a number through `format_number` → fails at
/// "not rounded".
#[test]
fn a_number_written_to_a_cell_keeps_its_digits() {
    let mut h = people();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    let write = |h: &mut TestHost, n: &str| {
        let src = format!("tables.People:set(1, {{ Name = {n} }})");
        run_script(h, &e, &src).unwrap();
        cells(h, "People")[0][0].clone()
    };
    assert_eq!(write(&mut h, "41.32765"), "41.32765", "not rounded");
    assert_eq!(write(&mut h, "0.0004"), "0.0004");
    assert_eq!(write(&mut h, "36"), "36");
    assert_eq!(write(&mut h, "-2.5"), "-2.5");
}

/// A table verb a run calls is seen by `tables`' later reads and kept by the
/// run's own writes after it; one called after the run wrote that table is
/// refused, since the run's end would write over it.
/// flip: `State::apply` not dropping the working copies a verb wrote → the first
/// run fails, its read of row 4 refused as past the stale count; `State::apply`
/// not refusing → the last run succeeds where it should fail.
#[test]
fn a_table_verb_in_a_run_is_read_after_and_never_written_over() {
    let mut h = people();
    let labels = FxHashMap::default();
    let e = env(&[], &labels);
    let src = "local p = tables.People\n\
               local before = p.rows\n\
               edit_table{ table = \"People\", add = {{ Name = \"Mae\" }} }\n\
               return before .. \" \" .. p.rows .. \" \" .. p[4].Name";
    let out = run_script(&mut h, &e, src).unwrap();
    assert_eq!(
        out.copied.as_deref(),
        Some("3 4 Mae"),
        "a read after the verb sees it"
    );
    let src = "local p = tables.People\n\
               local _ = p[1].Name\n\
               edit_table{ table = \"People\", columns = { add = {\"Age\"} } }\n\
               p:set(1, { Age = 36 })";
    run_script(&mut h, &e, src).unwrap();
    assert_eq!(
        cells(&h, "People")[0],
        ["Ada", "", "36"],
        "the verb's column and the run's write both land"
    );
    let before = cells(&h, "People");
    let src = "tables.People:set(1, { Name = \"X\" })\n\
               edit_table{ table = \"People\", add = {{ Name = \"Y\" }} }";
    let err = run_script(&mut h, &e, src).unwrap_err();
    assert_eq!(
        err.message,
        "edit_table changes People, which this run has written to: call it before the writes"
    );
    assert_eq!(cells(&h, "People"), before, "neither edit lands");
}
