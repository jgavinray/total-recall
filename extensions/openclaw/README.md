# OpenClaw Extension for Total Recall

This extension exposes Total Recall's episodic memory tools as native OpenClaw agents.

## What It Does

Provides three core tools for OpenClaw agents:

- **tr_store** - Store episodic memories with metadata (timestamp, location, context)
- **tr_search** - Search stored memories by query, date range, or metadata
- **tr_recall** - Recall memories based on semantic similarity or specific criteria

## Prerequisites

- Total Recall must be running with HTTP transport enabled:
  ```bash
  total-recall serve --transport http
  ```

## Installation

1. Copy this directory's contents to your OpenClaw extensions folder:
   ```bash
   cp -r /path/to/extensions/openclaw ~/.openclaw/extensions/total-recall-tools/
   ```

2. Ensure your `~/.openclaw/openclaw.json` includes the Total Recall config snippet (see below).

## Configuration

Add this to your OpenClaw configuration:

```json
{
  "total_recall": {
    "url": "http://localhost:3000",
    "transport": "http"
  }
}
```

## Tools

### tr_store

Stores episodic memories with optional metadata for future retrieval.

**Parameters:**
- `text` (required): The memory content to store
- `timestamp` (optional): Custom timestamp (defaults to now)
- `location` (optional): Physical or logical location
- `context` (optional): Additional context or tags

**Example:**
```
Store memory: "Met with team to discuss Q2 planning. Decided to prioritize OpenClaw integration."
Location: "Office, March 2026"
Context: "Q2 planning, OpenClaw"
```

### tr_search

Searches stored memories using query-based or metadata filters.

**Parameters:**
- `query` (required): Search query string
- `date_from` (optional): Start date for date range filter (YYYY-MM-DD)
- `date_to` (optional): End date for date range filter (YYYY-MM-DD)
- `location` (optional): Filter by location
- `context` (optional): Filter by context/tags

**Example:**
```
Search query: "Q2 planning"
Date range: 2026-03-01 to 2026-03-31
```

### tr_recall

Recalls memories based on semantic similarity or specific criteria.

**Parameters:**
- `query` (required): Query for semantic matching
- `limit` (optional): Maximum number of results to return
- `min_relevance` (optional): Minimum relevance threshold

**Example:**
```
Recall memories about "OpenClaw integration"
Limit: 5
```

## License

This extension is part of the Total Recall project. See the main repository for licensing information.
