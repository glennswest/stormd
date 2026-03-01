use crate::types::ScreenSnapshot;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Per-process VT100 terminal emulator.
pub struct ProcessTerminal {
    parser: vt100::Parser,
    process: String,
}

impl ProcessTerminal {
    pub fn new(process: impl Into<String>, rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            process: process.into(),
        }
    }

    /// Feed raw bytes from a child process into the VT100 parser.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Get the current screen text content.
    pub fn screen_text(&self) -> String {
        self.parser.screen().contents()
    }

    /// Get a full snapshot of the terminal screen.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let screen = self.parser.screen();
        ScreenSnapshot {
            process: self.process.clone(),
            rows: screen.size().0,
            cols: screen.size().1,
            contents: screen.contents(),
            cursor_row: screen.cursor_position().0,
            cursor_col: screen.cursor_position().1,
        }
    }

    /// Get the raw screen contents with ANSI escape codes for rendering.
    pub fn contents_formatted(&self) -> Vec<u8> {
        let screen = self.parser.screen();
        let mut out = Vec::new();
        // Row by row with formatting
        for row in 0..screen.size().0 {
            for col in 0..screen.size().1 {
                let cell = screen.cell(row, col).unwrap();
                out.extend_from_slice(cell.contents().as_bytes());
            }
            if row < screen.size().0 - 1 {
                out.extend_from_slice(b"\r\n");
            }
        }
        out
    }
}

/// Manages VT100 terminals for all processes.
pub struct TerminalManager {
    terminals: Mutex<HashMap<String, ProcessTerminal>>,
    default_rows: u16,
    default_cols: u16,
}

impl TerminalManager {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            default_rows: rows,
            default_cols: cols,
        }
    }

    /// Get or create a terminal for a process, then feed bytes into it.
    pub async fn feed(&self, process: &str, bytes: &[u8]) {
        let mut terminals = self.terminals.lock().await;
        let term = terminals
            .entry(process.to_string())
            .or_insert_with(|| ProcessTerminal::new(process, self.default_rows, self.default_cols));
        term.process(bytes);
    }

    /// Get a screen snapshot for a process.
    pub async fn snapshot(&self, process: &str) -> Option<ScreenSnapshot> {
        let terminals = self.terminals.lock().await;
        terminals.get(process).map(|t| t.snapshot())
    }

    /// Get screen text for a process.
    pub async fn screen_text(&self, process: &str) -> Option<String> {
        let terminals = self.terminals.lock().await;
        terminals.get(process).map(|t| t.screen_text())
    }

    /// Remove a process terminal.
    pub async fn remove(&self, process: &str) {
        let mut terminals = self.terminals.lock().await;
        terminals.remove(process);
    }

    /// List all processes with active terminals.
    pub async fn list_processes(&self) -> Vec<String> {
        let terminals = self.terminals.lock().await;
        terminals.keys().cloned().collect()
    }
}
