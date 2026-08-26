use anyhow::Result;
use serde::Deserialize;

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

// The component summary contract — the same shape /api/v1/components serves
// the web dashboard. See stormd's components.rs for the authoritative types.

#[derive(Debug, Clone, Deserialize)]
pub struct Metric {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub tone: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentAction {
    pub id: String,
    pub label: String,
    pub method: String,
    pub path: String,
    pub enabled: bool,
    pub danger: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentSummary {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub health: String,
    pub detail: String,
    #[serde(default)]
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub actions: Vec<ComponentAction>,
    #[serde(default)]
    pub link: Option<String>,
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

    pub async fn components(&self) -> Result<Vec<ComponentSummary>> {
        let resp = self
            .http
            .get(format!("{}/api/v1/components", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    /// Invoke a component action by the method+path the summary handed us.
    pub async fn invoke(&self, method: &str, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let req = match method {
            "GET" => self.http.get(url),
            _ => self.http.post(url),
        };
        req.send().await?.error_for_status()?;
        Ok(())
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
