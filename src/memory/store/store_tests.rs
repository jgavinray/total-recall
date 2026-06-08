use super::*;
use crate::config::EmbeddingConfig;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper: create a fresh MemoryStore backed by a temp directory.
/// Safe to unwrap: TempDir and MemoryStore::new are expected to succeed in a clean test env.
fn make_store() -> (TempDir, MemoryStore) {
    // Safe: tempdir() only fails on OS errors (out of space, permissions),
    // not expected in a normal test environment.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let model_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models/embed");
    let embedding_config = EmbeddingConfig {
        model_path: model_path.clone(),
        cache_dir: model_path,
        ..EmbeddingConfig::default()
    };
    // Safe: db_path is a fresh temp path with no contention.
    let store = MemoryStore::new_with_embedding_config(&db_path, &embedding_config).unwrap();
    (dir, store)
}

/// Sample note content with one observation.
fn sample_content() -> &'static str {
    "---\ntitle: Test Note\n---\n## Work\n## 10:00\n- [task] Buy milk #shopping\n"
}

// --- create_note tests ---

#[test]
fn test_create_note_returns_note() {
    let (_dir, store) = make_store();
    // Safe: make_store always succeeds in test env; fresh store has no date conflict.
    let note = store.create_note("2026-03-09", sample_content()).unwrap();
    assert_eq!(note.date, "2026-03-09");
    assert!(!note.id.is_empty());
    assert!(!note.archived);
}

#[test]
fn test_create_note_title_from_frontmatter() {
    let (_dir, store) = make_store();
    let note = store.create_note("2026-03-10", sample_content()).unwrap();
    assert_eq!(note.metadata.title, Some("Test Note".to_string()));
}

#[test]
fn test_create_note_duplicate_returns_error() {
    let (_dir, store) = make_store();
    // Safe: first insert must succeed; second on same date should fail with FileExistsError.
    store.create_note("2026-03-09", sample_content()).unwrap();
    let result = store.create_note("2026-03-09", sample_content());
    assert!(result.is_err(), "duplicate date should return an error");
}

// --- read_note tests ---

#[test]
fn test_read_note_after_create() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    // Safe: just created above, must exist.
    let note = store.read_note("2026-03-09").unwrap();
    assert_eq!(note.date, "2026-03-09");
    assert_eq!(note.content, sample_content());
}

#[test]
fn test_read_note_not_found_returns_error() {
    let (_dir, store) = make_store();
    let result = store.read_note("9999-99-99");
    assert!(result.is_err(), "reading non-existent note should fail");
}

#[test]
fn test_read_note_includes_observations() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    // Safe: note was just created above.
    let note = store.read_note("2026-03-09").unwrap();
    assert!(
        !note.observations.is_empty(),
        "note should have at least one observation"
    );
}

// --- archive / restore tests ---

#[test]
fn test_archive_note() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    // Safe: note exists; archive should succeed.
    store.archive_note("2026-03-09").unwrap();
    let note = store.read_note("2026-03-09").unwrap();
    assert!(
        note.archived,
        "note should be archived after archive_note()"
    );
}

#[test]
fn test_restore_note() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    // Safe: archive then restore; both operations on existing note.
    store.archive_note("2026-03-09").unwrap();
    store.restore_note("2026-03-09").unwrap();
    let note = store.read_note("2026-03-09").unwrap();
    assert!(
        !note.archived,
        "note should not be archived after restore_note()"
    );
}

// --- get_recent_notes tests ---

#[test]
fn test_get_recent_notes_returns_created() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    // Safe: limit=10, days=1 (note was just created with Utc::now() timestamp).
    let notes = store.get_recent_notes(10, 1, false).unwrap();
    assert!(!notes.is_empty(), "should return the recently created note");
    assert!(notes.iter().any(|n| n.date == "2026-03-09"));
}

#[test]
fn test_get_recent_notes_excludes_archived() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    store.archive_note("2026-03-09").unwrap();
    // Safe: note is archived; include_archived=false should exclude it.
    let notes = store.get_recent_notes(10, 1, false).unwrap();
    assert!(
        !notes.iter().any(|n| n.date == "2026-03-09"),
        "archived note should not appear when include_archived=false"
    );
}

#[test]
fn test_get_recent_notes_includes_archived_when_flag_set() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    store.archive_note("2026-03-09").unwrap();
    // Safe: include_archived=true, should include the archived note.
    let notes = store.get_recent_notes(10, 1, true).unwrap();
    assert!(
        notes.iter().any(|n| n.date == "2026-03-09"),
        "archived note should appear when include_archived=true"
    );
}

#[test]
fn test_get_recent_notes_respects_limit() {
    let (_dir, store) = make_store();
    for i in 1..=5 {
        let date = format!("2026-03-{:02}", i);
        store.create_note(&date, sample_content()).unwrap();
    }
    // Safe: 5 notes inserted; limit=3 should return at most 3.
    let notes = store.get_recent_notes(3, 1, false).unwrap();
    assert!(notes.len() <= 3, "should respect the limit parameter");
}

// --- search_notes tests ---

#[test]
fn test_search_notes_returns_relevant() {
    let (_dir, store) = make_store();
    store.create_note("2026-03-09", sample_content()).unwrap();
    let embedder = &store.embedder;
    let query_vec = embedder.embed("shopping task milk");
    // Safe: limit=5, non-archived search.
    let notes = store.search_notes(&query_vec, 5, false).unwrap();
    assert!(
        !notes.is_empty(),
        "semantic search should return at least one result"
    );
    assert!(
        notes.iter().any(|n| n.date == "2026-03-09"),
        "the note with matching content should appear in search results"
    );
}

#[test]
fn test_search_notes_empty_store() {
    let (_dir, store) = make_store();
    let embedder = &store.embedder;
    let query_vec = embedder.embed("anything");
    // Safe: no notes inserted; result should be empty, not an error.
    let notes = store.search_notes(&query_vec, 5, false).unwrap();
    assert!(
        notes.is_empty(),
        "search on empty store should return empty vec"
    );
}
