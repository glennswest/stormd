use anyhow::Result;
use serde::Deserialize;
use stormlog::types::LogEntry;

/// HTTP + WebSocket client for connecting to stormd.
pub struct StormClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessStatus {
    pub name: String,
    pub state: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub restarts: u32,
    pub uptime_secs: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

impl StormClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            base_url: format!("http://{}:{}", host, port),
            http: reqwest::Client::new(),
        }
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        let resp = self
            .http
            .get(format!("{}/api/v1/health", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    pub async fn processes(&self) -> Result<Vec<ProcessStatus>> {
        let resp = self
            .http
            .get(format!("{}/api/v1/processes", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    pub async fn start_process(&self, name: &str) -> Result<()> {
        self.http
            .post(format!("{}/api/v1/processes/{}/start", self.base_url, name))
            .send()
            .await?;
        Ok(())
    }

    pub async fn stop_process(&self, name: &str) -> Result<()> {
        self.http
            .post(format!("{}/api/v1/processes/{}/stop", self.base_url, name))
            .send()
            .await?;
        Ok(())
    }

    pub async fn restart_process(&self, name: &str) -> Result<()> {
        self.http
            .post(format!("{}/api/v1/processes/{}/restart", self.base_url, name))
            .send()
            .await?;
        Ok(())
    }

    pub async fn terminal_snapshot(&self, process: &str) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(format!("{}/api/v1/terminal/{}", self.base_url, process))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    /// Get the WebSocket URL for console streaming.
    pub fn ws_console_url(&self, process: &str) -> String {
        let ws_base = self.base_url.replace("http://", "ws://").replace("https://", "wss://");
        format!("{}/ws/console/{}", ws_base, process)
    }

    /// Get the WebSocket URL for log streaming.
    pub fn ws_logs_url(&self, process: Option<&str>) -> String {
        let ws_base = self.base_url.replace("http://", "ws://").replace("https://", "wss://");
        match process {
            Some(p) => format!("{}/ws/logs?process={}", ws_base, p),
            None => format!("{}/ws/logs", ws_base),
        }
    }
}
