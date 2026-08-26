use crate::client::{ComponentSummary, ProcessStatus, StormClient};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Dashboard,
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
    pub components: Vec<ComponentSummary>,
    pub selected_index: usize,
    pub dash_index: usize,
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
            view: View::Dashboard,
            input_mode: InputMode::Normal,
            running: true,
            processes: Vec::new(),
            components: Vec::new(),
            selected_index: 0,
            dash_index: 0,
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

    pub async fn refresh_components(&mut self) {
        if let Ok(components) = self.client.components().await {
            self.components = components;
            if self.dash_index >= self.components.len() && !self.components.is_empty() {
                self.dash_index = self.components.len() - 1;
            }
        }
    }

    pub fn dash_selected(&self) -> Option<&ComponentSummary> {
        self.components.get(self.dash_index)
    }

    /// Invoke the named action ("start", "stop", …) on the component selected
    /// in the dashboard, if the summary offers it and it is enabled.
    pub async fn dash_action(&mut self, action_id: &str) {
        let Some(component) = self.dash_selected() else { return };
        let label = component.label.clone();
        let Some(action) = component
            .actions
            .iter()
            .find(|a| a.id == action_id && a.enabled)
            .cloned()
        else {
            return;
        };
        match self.client.invoke(&action.method, &action.path).await {
            Ok(()) => self.status_message = format!("{} {}", action.label, label),
            Err(e) => self.status_message = format!("Error: {}", e),
        }
        self.refresh_components().await;
        self.refresh_processes().await;
    }

    pub fn dash_next(&mut self) {
        if !self.components.is_empty() {
            self.dash_index = (self.dash_index + 1) % self.components.len();
        }
    }

    pub fn dash_prev(&mut self) {
        if !self.components.is_empty() {
            self.dash_index = if self.dash_index == 0 {
                self.components.len() - 1
            } else {
                self.dash_index - 1
            };
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
