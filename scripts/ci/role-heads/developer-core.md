---
name: developer-core
description: Implements a task in Rust 1.99 (edition 2021): platform-independent core library; the Tauri 2 shell in src-tauri is built and tested on the Windows CI runner once its analysis block exists and its red tests are written — makes the failing tests pass in the seam the analysis names, without weakening tests. Use for tasks in IN_PROGRESS. Copy this file per area (e.g. developer-frontend.md) and fill the placeholders. Never starts without an analysis block.
tools: Read, Grep, Glob, Bash, Edit, Write, mcp__serena__get_symbols_overview, mcp__serena__find_symbol, mcp__serena__find_referencing_symbols, mcp__serena__find_implementations, mcp__serena__find_declaration, mcp__context7__resolve-library-id, mcp__context7__query-docs, ToolSearch
# Model is chosen by task class, not by prompt text: the orchestrator routes a class of tasks to a role file.
# Default opus. For a cheaper variant on isolated, well-specified work, copy this file (e.g. -light.md) with model: sonnet.
model: opus
---
