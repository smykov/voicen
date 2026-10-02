# Contract: `voicen_core::history` (core API and store trait)

Rust interface of the history module in `crates/voicen-core/src/history/`. Signatures are the contract; bodies are not. Tests on the Linux host use these exactly. Data shapes: [../data-model.md](../data-model.md).

## Types

```rust
pub enum EngineKind { Api, Local, Server }            // serde: "api" | "local" | "server"
pub struct EngineLabel { pub kind: EngineKind, pub model: String }

pub struct HistoryEntry { pub id: u64, pub text: String, pub ready_at_ms: i64, pub engine: EngineLabel }

pub enum HistoryNotice { SaveFailed, DeleteFailed }  // serde: "save_failed" | "delete_failed"

pub struct HistoryView { pub enabled: bool, pub entries: Vec<HistoryEntry>, pub notice: Option<HistoryNotice> }

/// Holds no text: only the operation, an io::ErrorKind or Corrupt, and counts (FR-011, R10).
pub struct HistoryError { pub op: HistoryOp, pub kind: HistoryErrorKind }
pub enum HistoryOp { Load, Save, Delete }
pub enum HistoryErrorKind { Io(std::io::ErrorKind), Corrupt }
```

`EngineKind` may become a re-export of 004/001's engine enum when that exists (P-010); the serialized names above are the contract.

## Store trait (the platform/disk seam)

```rust
pub trait HistoryStore: Send {
    /// Ok(None) when nothing is stored. Corrupt data => Err(kind = Corrupt). Never returns text in errors.
    fn load(&mut self) -> Result<Option<StoredHistory>, HistoryError>;
    /// Atomic replace: after Ok, exactly `doc` is stored; after Err, the previous document is intact.
    fn save(&mut self, doc: &StoredHistory) -> Result<(), HistoryError>;
    /// Removes the document and any interrupted partial write. Ok if nothing existed.
    fn delete(&mut self) -> Result<(), HistoryError>;
}

pub struct StoredHistory { pub next_id: u64, pub entries: Vec<HistoryEntry> }

/// `history.json` (+ `history.json.tmp`) in `dir`; `dir` comes from the shell's single data-directory resolver.
pub struct FileHistoryStore { /* dir: PathBuf */ }
impl FileHistoryStore { pub fn new(dir: PathBuf) -> Self; }
```

## History

```rust
pub struct History<S: HistoryStore> { /* … */ }

impl<S: HistoryStore> History<S> {
    /// Start-up (FR-009, FR-010): enabled=false => store.delete(), empty, never load.
    /// enabled=true => load; Corrupt/Io => empty + warning returned; trims to `size`.
    /// Returns the view plus at most one HistoryError to log.
    pub fn open(store: S, enabled: bool, size: u8) -> (Self, Option<HistoryError>);

    /// FR-001/FR-002: no-op when disabled or `text.trim()` is empty. Pushes front, trims to size,
    /// saves. Never fails the caller; a disk failure sets the notice and is returned for logging.
    pub fn record(&mut self, text: String, ready_at_ms: i64, engine: EngineLabel) -> Option<HistoryError>;

    /// FR-007: entries = [], delete (fallback: save empty). Works enabled or disabled.
    pub fn clear(&mut self) -> Option<HistoryError>;

    /// FR-003/FR-008, from 004's apply: off => clear + delete; on (from off) => empty, nothing written
    /// until the next record; size lowered => trim + save. Off wins over a size change in the same call.
    pub fn apply_settings(&mut self, enabled: bool, size: u8) -> Option<HistoryError>;

    /// FR-006: the full text of an entry, for the clipboard. None if the id is not in the list.
    pub fn text_of(&self, id: u64) -> Option<&str>;

    /// FR-005/FR-012: current view; never touches the store.
    pub fn view(&self) -> HistoryView;
}
```

### Behavioural guarantees (each has a red test first, P-004)

| # | Guarantee | Req |
|---|---|---|
| C1 | After any sequence of operations `view().entries.len() <= size` and the stored doc equals the view | FR-002, SC-002 |
| C2 | While disabled, the fake store sees no `save` call at all; `FileHistoryStore` dir contains no `history.json*` | FR-008, NFR-06 |
| C3 | `apply_settings(false, _)` deletes both files; a search of the dir for any prior text finds 0 matches | FR-008, SC-003 |
| C4 | `clear()` deletes; on delete failure it saves an empty doc; on both failing, notice = `DeleteFailed`, next op retries | FR-007, FR-009 |
| C5 | `open(enabled=false)` deletes leftovers (incl. `.tmp`) and never calls `load` | FR-009, FR-012 |
| C6 | Corrupt or unreadable file ⇒ empty history, `Corrupt`/`Io` error returned, no panic, no backup | FR-010 |
| C7 | A save failure leaves the entry in memory, notice = `SaveFailed`, `record` still returns normally | FR-010, SC-004 |
| C8 | `Display`/`Debug` of every `HistoryError` produced in tests never contains entry text | FR-011, SC-006 |
| C9 | Empty/whitespace text is never recorded | Edge case |
| C10 | Ids strictly increase across clear and off/on; order is insertion order regardless of `ready_at_ms` | Edge case (clock) |
| C11 | `FileHistoryStore::save` interrupted (tmp written, rename not done) ⇒ `load` returns the previous doc | FR-004 |
| C12 | No network: the module has no HTTP dependency (checked by dependency review) | FR-014 |
