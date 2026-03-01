use crate::client::{ProcessStatus, StormClient};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Processes,
    Terminal,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Normal,
    Search,
}

pub struct App {
    pub client: StormClient,
    pub view: View,
    pub input_mode: InputMode,
    pub running: bool,
    pub processes: Vec<ProcessStatus>,
    pub selected_index: usize,
    pub log_lines: Arc<Mutex<Vec<String>>>,
    pub terminal_content: String,
    pub search_query: String,
    pub status_message: String,
    pub connected: bool,
}

impl App {
    pub fn new(client: StormClient) -> Self {
        Self {
            client,
            view: View::Processes,
            input_mode: InputMode::Normal,
            running: true,
            processes: Vec::new(),
            selected_index: 0,
            log_lines: Arc::new(Mutex::new(Vec::new())),
            terminal_content: String::new(),
            search_query: String::new(),
            status_message: String::new(),
            connected: false,
        }
    }

    pub async fn refresh_processes(&mut self) {
        match self.client.processes().await {
            Ok(procs) => {
                self.processes = procs;
                self.connected = true;
                if self.selected_index >= self.processes.len() && !self.processes.is_empty() {
                    self.selected_index = self.processes.len() - 1;
                }
            }
            Err(e) => {
                self.connected = false;
                self.status_message = format!("Connection error: {}", e);
            }
        }
    }

    pub fn selected_process(&self) -> Option<&ProcessStatus> {
        self.processes.get(self.selected_index)
    }

    pub fn select_next(&mut self) {
        if !self.processes.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.processes.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.processes.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.processes.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }
}
