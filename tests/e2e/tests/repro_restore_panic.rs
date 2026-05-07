// Ad-hoc repro for the `restore-at` panic seen on demo/.agentsync/doc.bin.
// Run individual tests with `cargo test -p agentsync-e2e --test
// repro_restore_panic <name> -- --nocapture --ignored`.

use agentsync_core::Doc;
use std::fs;
use std::path::PathBuf;

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../demo/.agentsync/doc.bin")
        .canonicalize()
        .expect("demo doc.bin")
}

#[test]
#[ignore]
fn load_only() {
    let bytes = fs::read(doc_path()).unwrap();
    let _doc = Doc::load(&bytes).expect("load");
    println!("load OK ({} bytes)", bytes.len());
}

#[test]
#[ignore]
fn load_and_save() {
    let bytes = fs::read(doc_path()).unwrap();
    let mut doc = Doc::load(&bytes).expect("load");
    let out = doc.save();
    println!("save OK ({} bytes)", out.len());
}

#[test]
#[ignore]
fn load_and_restore_to_time() {
    let bytes = fs::read(doc_path()).unwrap();
    let mut doc = Doc::load(&bytes).expect("load");
    println!("calling restore_to_time(1778049392443)");
    doc.restore_to_time(1778049392443).expect("restore_to_time");
    println!("restore OK");
    println!("calling save");
    let out = doc.save();
    println!("save OK ({} bytes)", out.len());
}

#[test]
#[ignore]
fn dump_changes() {
    let bytes = fs::read(doc_path()).unwrap();
    let mut doc = Doc::load(&bytes).expect("load");
    let changes = doc.debug_changes();
    println!("change count = {}", changes.len());
    let target: i64 = 1778049392443;
    let mut idx_by_hash = std::collections::HashMap::new();
    for (i, (h, _, _)) in changes.iter().enumerate() {
        idx_by_hash.insert(*h, i);
    }
    for (i, (h, ts, deps)) in changes.iter().enumerate() {
        let dep_idxs: Vec<String> = deps
            .iter()
            .map(|d| {
                idx_by_hash
                    .get(d)
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| format!("?{}", short(d)))
            })
            .collect();
        println!(
            "[{:>2}] ts={} (rel target: {:+}) hash={} deps=[{}]",
            i,
            ts,
            ts - target,
            short(h),
            dep_idxs.join(",")
        );
    }
    let included: usize = changes.iter().filter(|c| c.1 <= target).count();
    println!("changes with ts <= target ({}): {}", target, included);
}

fn short(h: &automerge::ChangeHash) -> String {
    let bytes = h.as_ref();
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}
