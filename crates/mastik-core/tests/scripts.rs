//! Document scripts (`script`): the list's operations and their undo, a source
//! edit folding into a run, the save format, and the three verbs.

use mastik_core::verbs::{self, Ctx, Done, VerbError};
use mastik_core::{
    Document, History, IdSource, NodeId, OpError, Operation, Resolved, Script, ScriptId,
    Transaction, io,
};
use serde_json::{Value, json};

fn blank() -> (Document, IdSource) {
    let mut ids = IdSource::new(0x5C);
    let page = ids.mint();
    (Document::new(page), ids)
}

fn script(ids: &mut IdSource, name: &str, source: &str) -> Script {
    Script {
        id: ids.mint_script(),
        name: name.into(),
        source: source.into(),
    }
}

fn add(script: &Script, index: usize) -> Transaction {
    Transaction(vec![Operation::AddScript {
        script: script.clone(),
        index,
    }])
}

fn names(doc: &Document) -> Vec<&str> {
    doc.scripts().iter().map(|s| s.name.as_str()).collect()
}

/// Each operation is undone to exactly the document before it and redone to
/// exactly the one after, a removal putting the script back in its place.
///
/// flip: `RemoveScript`'s inverse carrying `index: 0` → fails at "undo restores"
/// for the removal of `b`.
#[test]
fn every_script_operation_undoes_and_redoes_exactly() {
    let (mut doc, mut ids) = blank();
    let (a, b, c) = (
        script(&mut ids, "A", "print(1)"),
        script(&mut ids, "B", "print(2)"),
        script(&mut ids, "C", "print(3)"),
    );
    let mut history = History::new();
    for (s, i) in [(&a, 0), (&c, 1), (&b, 1)] {
        history.commit(&mut doc, add(s, i)).unwrap();
    }
    assert_eq!(names(&doc), ["A", "B", "C"], "added at their indices");

    let steps = [
        Operation::RemoveScript { id: b.id },
        Operation::SetScriptName {
            id: c.id,
            name: "Tidy".into(),
        },
        Operation::SetScriptSource {
            id: a.id,
            source: "sel.x = 0".into(),
        },
    ];
    for op in steps {
        let before = doc.clone();
        history
            .commit(&mut doc, Transaction(vec![op.clone()]))
            .unwrap();
        let after = doc.clone();
        assert_ne!(after, before, "{op:?} changed something");
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before, "{op:?}: undo restores");
        history.redo(&mut doc).unwrap();
        assert_eq!(doc, after, "{op:?}: redo restores");
    }
    assert_eq!(names(&doc), ["A", "Tidy"]);
    assert_eq!(doc.script(a.id).unwrap().source, "sel.x = 0");
}

/// A duplicate id, a missing script and an index past the end are refused, and
/// the document is left as it was.
#[test]
fn a_bad_script_operation_is_refused() {
    let (mut doc, mut ids) = blank();
    let a = script(&mut ids, "A", "");
    doc.apply(&add(&a, 0)).unwrap();
    let before = doc.clone();
    assert!(matches!(
        doc.apply(&add(&a, 0)),
        Err(OpError::DuplicateScript(id)) if id == a.id
    ));
    let ghost = ids.mint_script();
    assert!(matches!(
        doc.apply(&Transaction(vec![Operation::RemoveScript { id: ghost }])),
        Err(OpError::NoSuchScript(id)) if id == ghost
    ));
    let b = script(&mut ids, "B", "");
    assert!(matches!(
        doc.apply(&add(&b, 2)),
        Err(OpError::IndexOutOfRange)
    ));
    assert_eq!(doc, before);
}

/// Typing into a script — one source edit per keystroke, committed into a run —
/// is one undo step, and that step goes back to the source before the run.
///
/// flip: `shape_key` without its `SetScriptSource` arm → fails at "one step".
#[test]
fn source_edits_fold_into_one_step() {
    let (mut doc, mut ids) = blank();
    let a = script(&mut ids, "A", "");
    let mut history = History::new();
    history.commit(&mut doc, add(&a, 0)).unwrap();
    history.end_run();
    let typed = "print(1)";
    for n in 1..=typed.len() {
        let tx = Transaction(vec![Operation::SetScriptSource {
            id: a.id,
            source: typed[..n].into(),
        }]);
        history.commit_into_run(&mut doc, tx).unwrap();
    }
    assert_eq!(doc.script(a.id).unwrap().source, typed);
    assert_eq!(
        history.undo_depth(),
        2,
        "the add, then the typing as one step"
    );
    history.undo(&mut doc).unwrap();
    assert_eq!(
        doc.script(a.id).unwrap().source,
        "",
        "back to before the run"
    );

    // Another script's edit does not join the first's run.
    let b = script(&mut ids, "B", "");
    history.commit(&mut doc, add(&b, 1)).unwrap();
    for id in [a.id, b.id] {
        let tx = Transaction(vec![Operation::SetScriptSource {
            id,
            source: "x".into(),
        }]);
        history.commit_into_run(&mut doc, tx).unwrap();
    }
    assert_eq!(history.undo_depth(), 4, "one step per script");
}

/// Scripts save in list order and load back equal; a document without any saves
/// without the key, so its bytes are what they were before scripts existed.
///
/// flip: `DocumentDto::scripts` without its `skip_serializing_if` → fails at
/// "no key".
#[test]
fn scripts_round_trip_and_an_empty_list_is_not_written() {
    let (mut doc, mut ids) = blank();
    let empty = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    assert!(!empty.contains("\"scripts\""), "no key:\n{empty}");

    let (b, a) = (
        script(&mut ids, "Zeta", "-- runs by hand\nprint(\"hi\")"),
        script(&mut ids, "Alpha", ""),
    );
    doc.apply(&add(&b, 0)).unwrap();
    doc.apply(&add(&a, 1)).unwrap();
    let bytes = io::save(&doc).unwrap();
    let back = io::load(&bytes).unwrap();
    assert_eq!(back, doc);
    assert_eq!(names(&back), ["Zeta", "Alpha"], "list order, not id order");
    assert_eq!(io::save(&back).unwrap(), bytes, "re-saved byte for byte");

    // A loaded script's id is reserved past, as a style's is.
    let mut fresh = IdSource::new(0x5C);
    mastik_core::reserve_existing_ids(&back, &mut fresh);
    assert!(fresh.mint() > a.id.0);
}

/// Two branches each adding a script and one editing a shared one merge into a
/// document holding all three, matched by id.
#[test]
fn scripts_merge_by_id() {
    let (mut base, mut ids) = blank();
    let shared = script(&mut ids, "Shared", "print(0)");
    base.apply(&add(&shared, 0)).unwrap();
    let (mut ours, mut theirs) = (base.clone(), base.clone());
    let (a, b) = (script(&mut ids, "Ours", ""), script(&mut ids, "Theirs", ""));
    ours.apply(&add(&a, 1)).unwrap();
    theirs.apply(&add(&b, 1)).unwrap();
    theirs
        .apply(&Transaction(vec![Operation::SetScriptSource {
            id: shared.id,
            source: "print(1)".into(),
        }]))
        .unwrap();
    let save = |d: &Document| io::save(d).unwrap();
    let merged = mastik_core::merge::merge(&save(&base), &save(&ours), &save(&theirs)).unwrap();
    assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);
    let doc = io::load(&merged.bytes).unwrap();
    assert_eq!(names(&doc), ["Shared", "Ours", "Theirs"]);
    assert_eq!(doc.script(shared.id).unwrap().source, "print(1)");
}

/// A version compare names a script's change by the script's name, which the
/// keyed-list compare reads off the item itself.
#[test]
fn a_script_change_reads_by_name() {
    let (mut before, mut ids) = blank();
    let s = script(&mut ids, "Tidy", "print(0)");
    before.apply(&add(&s, 0)).unwrap();
    let mut after = before.clone();
    after
        .apply(&Transaction(vec![Operation::SetScriptSource {
            id: s.id,
            source: "print(1)".into(),
        }]))
        .unwrap();
    let d = mastik_core::diff::diff(&before, &after);
    let lines: Vec<String> = d.document.iter().map(|c| c.line()).collect();
    assert_eq!(lines, ["script “Tidy” source: print(0) → print(1)"]);
}

/// A file naming one script id twice is refused.
#[test]
fn a_duplicate_script_id_does_not_load() {
    let (mut doc, mut ids) = blank();
    let a = script(&mut ids, "A", "");
    doc.apply(&add(&a, 0)).unwrap();
    let mut file: Value = serde_json::from_slice(&io::save(&doc).unwrap()).unwrap();
    let one = file["scripts"][0].clone();
    file["scripts"].as_array_mut().unwrap().push(one);
    let err = io::load(&serde_json::to_vec(&file).unwrap()).unwrap_err();
    assert!(err.to_string().contains("duplicate script id"), "{err}");
}

fn call(doc: &Document, ids: &mut IdSource, name: &str, args: Value) -> Result<Done, VerbError> {
    let res = Resolved::rebuild(doc);
    verbs::call(
        &mut Ctx {
            doc,
            res: &res,
            ids,
            images: None,
            contrast: None,
        },
        name,
        &args,
    )
}

/// Run `name` and commit its edit, checking its undo and redo land exactly.
fn run(doc: &mut Document, ids: &mut IdSource, name: &str, args: Value) -> Option<String> {
    let done = call(doc, ids, name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
    let before = doc.clone();
    let undo = doc.apply(&done.tx).unwrap().inverse;
    let after = doc.clone();
    let redo = doc.apply(&undo).unwrap().inverse;
    assert_eq!(*doc, before, "{name}: undo restores");
    doc.apply(&redo).unwrap();
    assert_eq!(*doc, after, "{name}: redo restores");
    done.made
}

/// `add_script` answers the new script's id, `set_script` reaches it by id or by
/// name, and `remove_script` takes it out.
///
/// flip: `id_named` without its `script` arm → fails at the `set_script` by name.
#[test]
fn the_script_verbs_add_set_and_remove() {
    let (mut doc, mut ids) = blank();
    let made = run(
        &mut doc,
        &mut ids,
        "add_script",
        json!({ "name": " Card grid ", "source": "print(1)" }),
    )
    .expect("answers the id");
    let id = ScriptId(NodeId::from_wire(&made).unwrap());
    let s = doc.script(id).expect("the id it answered");
    assert_eq!(
        (s.name.as_str(), s.source.as_str()),
        ("Card grid", "print(1)")
    );

    run(
        &mut doc,
        &mut ids,
        "set_script",
        json!({ "script": made, "source": "print(2)" }),
    );
    run(
        &mut doc,
        &mut ids,
        "set_script",
        json!({ "script": "Card grid", "name": "Grid" }),
    );
    let s = doc.script(id).unwrap();
    assert_eq!((s.name.as_str(), s.source.as_str()), ("Grid", "print(2)"));

    for (args, param) in [
        (json!({ "script": made }), "script"),
        (json!({ "script": made, "name": "  " }), "name"),
        (json!({ "script": "Nope", "name": "x" }), "script"),
    ] {
        match call(&doc, &mut ids, "set_script", args.clone()) {
            Err(VerbError::Invalid { param: p, .. }) => assert_eq!(p, param, "{args}"),
            other => panic!("{args}: {other:?}"),
        }
    }

    run(
        &mut doc,
        &mut ids,
        "remove_script",
        json!({ "script": made }),
    );
    assert!(doc.scripts().is_empty());
}
