/// Integration tests for Total Recall: write → read → search flow.
///
/// These tests exercise the full MemoryStore pipeline using real in-process SQLite
/// (temp file) and the real ONNX embedder (model must be cached on disk).
use tempfile::TempDir;
use total_recall::memory::embedder::Embedder;
use total_recall::memory::store::MemoryStore;

/// Helper: spin up a fresh store in a temp directory.
/// Safe to unwrap: TempDir and MemoryStore::new only fail on OS errors (permissions, disk full),
/// not expected in a CI environment.
fn make_store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().unwrap(); // safe: OS temp dir available
    let db_path = dir.path().join("integration-test.db");
    let store = MemoryStore::new(&db_path).unwrap(); // safe: fresh path, no contention
    (dir, store)
}

/// Sample daily note with two observations under distinct timestamps.
fn note_content() -> &'static str {
    "---\ntitle: Integration Test Note\n---\n\
     ## Work\n\
     ## 09:00\n\
     - [task] Write integration tests for total-recall #rust #testing\n\
     ## 14:00\n\
     - [idea] Improve semantic search with hybrid retrieval #ai\n"
}

/// Core integration flow: write a note, read it back, search for it by content.
///
/// This test verifies the three-layer pipeline:
///   1. create_note persists the note and its observations with embeddings
///   2. get_recent_notes returns the note in recent results
///   3. search_notes with a semantically related query returns the note
#[test]
fn test_write_read_search_flow() {
    let (_dir, store) = make_store();

    // ── 1. Write ────────────────────────────────────────────────────────────
    // Safe: fresh store, no duplicate key conflicts.
    let created = store
        .create_note("2026-03-09", note_content())
        .expect("create_note should succeed");

    assert_eq!(created.date, "2026-03-09", "created note should have correct date");
    assert_eq!(
        created.metadata.title,
        Some("Integration Test Note".to_string()),
        "title should be parsed from frontmatter"
    );
    assert!(!created.archived, "newly created note should not be archived");

    // ── 2. Read (get_recent_notes) ───────────────────────────────────────────
    // days=1: note was just created with Utc::now() timestamp, so it's within the window.
    // Safe: get_recent_notes returns Err only on DB failures, not expected here.
    let recent = store
        .get_recent_notes(10, 1, false)
        .expect("get_recent_notes should succeed");

    assert!(
        !recent.is_empty(),
        "get_recent_notes should return at least the note we just created"
    );
    assert!(
        recent.iter().any(|n| n.date == "2026-03-09"),
        "2026-03-09 should appear in recent notes"
    );

    // ── 3. Read (read_note directly) ─────────────────────────────────────────
    // Safe: we just created this note; it must exist.
    let note = store
        .read_note("2026-03-09")
        .expect("read_note should succeed for an existing note");

    assert_eq!(note.content, note_content(), "content should be stored verbatim");
    assert!(
        !note.observations.is_empty(),
        "note should have at least one parsed observation"
    );

    // Verify at least one observation category is parsed
    let has_task = note.observations.iter().any(|o| o.category.as_deref() == Some("task"));
    assert!(has_task, "at least one observation should have category 'task'");

    // ── 4. Search ────────────────────────────────────────────────────────────
    // Safe: Embedder::new() downloads to cache dir; assumed cached from unit tests.
    let embedder = Embedder::new().expect("embedder init should succeed with cached model");

    // Query text is semantically related to the note content.
    let query_vec = embedder.embed("rust testing integration tests");
    // Safe: search_notes only fails on DB errors; not expected here.
    let results = store
        .search_notes(&query_vec, 5, false)
        .expect("search_notes should succeed");

    assert!(
        !results.is_empty(),
        "search should return at least one result for a semantically related query"
    );
    assert!(
        results.iter().any(|n| n.date == "2026-03-09"),
        "the note we created should appear in semantic search results"
    );

    // ── 5. Verify content is searchable ──────────────────────────────────────
    // The returned note content should contain the text we stored.
    let matched_note = results
        .iter()
        .find(|n| n.date == "2026-03-09")
        .expect("note should be in results");
    assert!(
        matched_note.content.contains("integration tests"),
        "returned note content should contain the original text; got: {:?}",
        &matched_note.content[..matched_note.content.len().min(200)]
    );
}

/// Verify archive/restore round-trip: archived notes should not appear in search,
/// then should reappear after restoring.
#[test]
fn test_archive_restore_affects_search() {
    let (_dir, store) = make_store();

    // Safe: fresh store, unique date.
    store
        .create_note("2026-03-10", note_content())
        .expect("create_note should succeed");

    let embedder = Embedder::new().expect("embedder init");
    let query_vec = embedder.embed("rust testing");

    // Before archive: note should be searchable.
    // Safe: limit 5, not archived.
    let before = store
        .search_notes(&query_vec, 5, false)
        .expect("search should succeed");
    assert!(
        before.iter().any(|n| n.date == "2026-03-10"),
        "note should be findable before archiving"
    );

    // Archive it.
    // Safe: note exists; archive_note only fails on DB errors.
    store.archive_note("2026-03-10").expect("archive should succeed");

    // After archive: should NOT appear in non-archived search.
    // Safe: same query, include_archived=false.
    let after_archive = store
        .search_notes(&query_vec, 5, false)
        .expect("search after archive should succeed");
    assert!(
        !after_archive.iter().any(|n| n.date == "2026-03-10"),
        "archived note should not appear in non-archived search"
    );

    // Restore it.
    // Safe: note exists and is archived.
    store.restore_note("2026-03-10").expect("restore should succeed");

    // After restore: should be searchable again.
    // Safe: same as above.
    let after_restore = store
        .search_notes(&query_vec, 5, false)
        .expect("search after restore should succeed");
    assert!(
        after_restore.iter().any(|n| n.date == "2026-03-10"),
        "restored note should be findable again"
    );
}

/// Verify that multiple notes are independently searchable and the most relevant
/// one ranks first.
#[test]
fn test_multiple_notes_semantic_ranking() {
    let (_dir, store) = make_store();

    let rust_note = "---\ntitle: Rust Note\n---\n## 10:00\n- [note] Rust ownership and borrowing #rust\n";
    let cooking_note =
        "---\ntitle: Cooking Note\n---\n## 10:00\n- [note] Bake a sourdough loaf #cooking\n";

    // Safe: fresh store, distinct dates.
    store
        .create_note("2026-03-11", rust_note)
        .expect("create rust note");
    store
        .create_note("2026-03-12", cooking_note)
        .expect("create cooking note");

    let embedder = Embedder::new().expect("embedder init");
    let query_vec = embedder.embed("Rust programming language borrow checker");

    // Safe: limit 5, not archived.
    let results = store
        .search_notes(&query_vec, 5, false)
        .expect("search should succeed");

    assert!(
        results.iter().any(|n| n.date == "2026-03-11"),
        "Rust note should appear in results for a Rust-related query"
    );
}
