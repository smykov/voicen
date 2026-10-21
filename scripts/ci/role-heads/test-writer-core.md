---
name: test-writer-core
description: Writes red tests for a task in Rust 1.99 (edition 2021): platform-independent core library; the Tauri 2 shell in src-tauri is built and tested on the Windows CI runner before implementation — characterization tests first where the seam has none, then one red test per guarantee in the DoD, failure branches included — a reproducing red test for every bug or rca fix, and the end-to-end test (API or UI) for the Acceptance failure branch that verification will run. Use after the analysis is complete and the task has entered IN_PROGRESS (tests are code to the analysis gate), and before developer. Copy per area and fill the placeholders. Does not write implementation code.
tools: Read, Grep, Glob, Bash, Edit, Write, mcp__serena__get_symbols_overview, mcp__serena__find_symbol, mcp__serena__find_referencing_symbols, mcp__serena__find_implementations, mcp__serena__find_declaration, mcp__context7__resolve-library-id, mcp__context7__query-docs, ToolSearch
# Model is chosen by task class, not by prompt text: the orchestrator routes a class of tasks to a role file.
# Default opus. For a cheaper variant on isolated, well-specified work, copy this file (e.g. -light.md) with model: sonnet.
model: opus
---
