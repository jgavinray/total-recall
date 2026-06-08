use total_recall::{config, error, memory};

pub(crate) async fn run_write(
    config: &config::Config,
    content: &str,
    timestamp: Option<&str>,
    append: bool,
) -> anyhow::Result<()> {
    let store =
        memory::store::MemoryStore::new_with_embedding_config(&config.db_path, &config.embedding)
            .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
    let final_content = if let Some(ts) = timestamp {
        format!("## {}\n\n{}", ts, content)
    } else {
        format!("\n{}", content)
    };

    if append {
        match store.append_note(&current_date, &final_content) {
            Ok(note) => println!("Appended to note for {}", note.date),
            Err(e) => {
                eprintln!("Error appending to note: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        match store.create_note(&current_date, &final_content) {
            Ok(note) => {
                println!("Created note for {}", note.date);
                println!(
                    "Title: {}",
                    note.metadata.title.as_deref().unwrap_or("Untitled")
                );
            }
            Err(error::MemoryError::FileExistsError(_)) => {
                match store.append_note(&current_date, &final_content) {
                    Ok(note) => println!("Appended to existing note for {}", note.date),
                    Err(e) => {
                        eprintln!("Error appending to note: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error creating note: {}", e);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

pub(crate) async fn run_read(config: &config::Config, date: &str) -> anyhow::Result<()> {
    let store =
        memory::store::MemoryStore::new_with_embedding_config(&config.db_path, &config.embedding)
            .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    match store.read_note(date) {
        Ok(note) => {
            println!("## {}\n", note.date);
            println!("{}", note.content);
        }
        Err(error::MemoryError::NotFound(_)) => {
            eprintln!("No note found for date: {}", date);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error reading note: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

pub(crate) async fn run_search(
    config: &config::Config,
    query: &str,
    limit: usize,
    include_archived: bool,
) -> anyhow::Result<()> {
    let embedder = memory::embedder::Embedder::from_config(&config.embedding)
        .map_err(|e| anyhow::anyhow!("Failed to initialize embedder: {}", e))?;
    let store =
        memory::store::MemoryStore::new_with_embedding_config(&config.db_path, &config.embedding)
            .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    let query_embedding = embedder.embed(query);
    match store.search_notes(&query_embedding, limit, include_archived) {
        Ok(notes) => {
            if notes.is_empty() {
                println!("No notes found matching your query.");
            } else {
                for note in &notes {
                    let title = note.metadata.title.as_deref().unwrap_or("Untitled");
                    println!("### {} - {}\n", note.date, title);
                    let content_preview = note.content.chars().take(200).collect::<String>();
                    println!("{}\n", content_preview);
                }
            }
        }
        Err(e) => {
            eprintln!("Error searching: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

pub(crate) async fn run_recent(
    config: &config::Config,
    limit: usize,
    days: usize,
    include_archived: bool,
) -> anyhow::Result<()> {
    let store =
        memory::store::MemoryStore::new_with_embedding_config(&config.db_path, &config.embedding)
            .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    match store.get_recent_notes(limit, days, include_archived) {
        Ok(notes) => {
            if notes.is_empty() {
                println!("No notes found in the last {} days.", days);
            } else {
                println!("Recent notes (last {} days):\n", days);
                for note in &notes {
                    let title = note.metadata.title.as_deref().unwrap_or(&note.date);
                    println!("- **{}** ({})", note.date, title);
                }
            }
        }
        Err(e) => {
            eprintln!("Error getting recent notes: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
