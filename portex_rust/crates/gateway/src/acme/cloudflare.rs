//! Minimal Cloudflare DNS API client — only the calls we need to satisfy
//! the ACME DNS-01 challenge: add a TXT record, list TXT records by name,
//! delete a record by ID.
//!
//! Docs: https://developers.cloudflare.com/api/operations/dns-records-for-a-zone-list-dns-records

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

const API_BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct Cloudflare {
    http: Client,
    token: String,
    zone_id: String,
}

impl Cloudflare {
    pub fn new(token: String, zone_id: String) -> Self {
        Self {
            http: Client::builder().user_agent("portex-gateway/0.2").build().expect("reqwest"),
            token,
            zone_id,
        }
    }

    /// Create a TXT record. Returns the record ID so we can delete it later.
    /// `name` is the full DNS name (e.g. `_acme-challenge.portex.live`).
    pub async fn create_txt(&self, name: &str, value: &str, ttl: u32) -> Result<String> {
        let url = format!("{API_BASE}/zones/{}/dns_records", self.zone_id);
        let body = json!({
            "type": "TXT",
            "name": name,
            "content": value,
            "ttl": ttl,
        });
        let resp: CloudflareResp<RecordEnvelope> = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("cloudflare create_txt request")?
            .json()
            .await
            .context("cloudflare create_txt parse")?;
        resp.ok()?;
        let id = resp.result.context("missing result")?.id;
        tracing::info!(record_id = %id, %name, "cloudflare: TXT created");
        Ok(id)
    }

    pub async fn delete_record(&self, record_id: &str) -> Result<()> {
        let url = format!("{API_BASE}/zones/{}/dns_records/{record_id}", self.zone_id);
        let resp: CloudflareResp<RecordEnvelope> = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("cloudflare delete_record request")?
            .json()
            .await
            .context("cloudflare delete_record parse")?;
        resp.ok()?;
        tracing::info!(record_id = %record_id, "cloudflare: TXT deleted");
        Ok(())
    }

    /// Verify our credentials + zone before we trigger ACME.
    pub async fn verify(&self) -> Result<()> {
        let url = format!("{API_BASE}/zones/{}", self.zone_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("cloudflare verify request")?;
        if !resp.status().is_success() {
            anyhow::bail!("cloudflare zone verify failed: {}", resp.status());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct CloudflareResp<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<CloudflareError>,
    #[serde(default = "Option::default")]
    result: Option<T>,
}

impl<T> CloudflareResp<T> {
    fn ok(&self) -> Result<()> {
        if self.success {
            return Ok(());
        }
        let msg = self
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.code, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!("cloudflare API error: {msg}")
    }
}

#[derive(Debug, Deserialize)]
struct CloudflareError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecordEnvelope {
    id: String,
}
