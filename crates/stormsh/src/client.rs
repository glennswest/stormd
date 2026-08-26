use anyhow::Result;
use serde::Deserialize;

/// HTTP + WebSocket client for connecting to stormd. When stormd has auth
/// enabled (`[api] auth_token` / `password`), pass the token — every request
/// carries it as a bearer credential.
pub struct StormClient {
    base_url: String,
    http: reqwest::Client,
    token: Option<String>,
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

// The component summary shapes come from the shared stormview crate — the
// same types stormd serializes, so the two cannot disagree about the wire.
pub use stormview::ComponentSummary;

impl StormClient {
    pub fn new(host: &str, port: u16, token: Option<String>) -> Self {
        Self {
            base_url: format!("http://{}:{}", host, port),
            http: reqwest::Client::new(),
            token,
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.authed(self.http.get(format!("{}{}", self.base_url, path)))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.authed(self.http.post(format!("{}{}", self.base_url, path)))
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        Ok(self.get("/api/v1/health").send().await?.json().await?)
    }

    pub async fn processes(&self) -> Result<Vec<ProcessStatus>> {
        Ok(self
            .get("/api/v1/processes")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn components(&self) -> Result<Vec<ComponentSummary>> {
        Ok(self
            .get("/api/v1/components")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Invoke a component action by the method+path the summary handed us.
    pub async fn invoke(&self, method: &str, path: &str) -> Result<()> {
        let req = match method {
            "GET" => self.get(path),
            _ => self.post(path),
        };
        req.send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn start_process(&self, name: &str) -> Result<()> {
        self.post(&format!("/api/v1/processes/{}/start", name))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn stop_process(&self, name: &str) -> Result<()> {
        self.post(&format!("/api/v1/processes/{}/stop", name))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn restart_process(&self, name: &str) -> Result<()> {
        self.post(&format!("/api/v1/processes/{}/restart", name))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn terminal_snapshot(&self, process: &str) -> Result<serde_json::Value> {
        Ok(self
            .get(&format!("/api/v1/terminal/{}", process))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
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
