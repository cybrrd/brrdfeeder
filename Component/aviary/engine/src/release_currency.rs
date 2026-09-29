//! Public host-poller receipt projection. No scheduling or network activity.
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReleaseCurrency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_release_check_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_seq_seen: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_release_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_outcome: Option<String>,
}
#[derive(Deserialize)]
struct Receipt {
    node_id: String,
    engine_digest: String,
    #[serde(flatten)]
    currency: ReleaseCurrency,
}
impl ReleaseCurrency {
    pub fn read(path: &Path, node: &str, running_digest: Option<&str>) -> Self {
        Self::try_read(path,node,running_digest).unwrap_or_default()
    }
    fn try_read(path: &Path, node: &str, running_digest: Option<&str>) -> Option<Self> {
        let file=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK).open(path).ok()?;
        let meta=file.metadata().ok()?;
        if !meta.is_file()||meta.len()>4096{return None;}
        let mut raw=Vec::new();file.take(4097).read_to_end(&mut raw).ok()?;
        let mut r:Receipt=serde_json::from_slice(&raw).ok()?;
        if r.node_id!=node{return None;}
        if let Some(time)=&r.currency.last_release_check_at {
            let parsed=chrono::DateTime::parse_from_rfc3339(time).ok()?;
            if parsed.timestamp_millis()>chrono::Utc::now().timestamp_millis(){return None;}
        }
        if !matches!(r.currency.update_outcome.as_deref(),None|Some(""|"applied"|"rolled_back"|"quarantined"|"rollback_failed")){return None;}
        // A saved target is not evidence of what this engine is running.
        if running_digest!=Some(r.engine_digest.as_str())||r.engine_digest.is_empty(){r.currency.running_release_seq=None;}
        Some(r.currency)
    }
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn currency_is_identity_bound_and_missing_is_unknown(){
  let root=std::env::temp_dir().join(format!("currency-{}",std::process::id()));
  std::fs::create_dir_all(&root).unwrap();let path=root.join("currency.json");
  assert!(ReleaseCurrency::read(&path,"test",Some("digest")).release_seq_seen.is_none());
  std::fs::write(&path,br#"{"node_id":"test","engine_digest":"digest","last_release_check_at":"2026-01-01T00:00:00Z","release_seq_seen":8,"running_release_seq":7,"update_outcome":"rolled_back"}"#).unwrap();
  let c=ReleaseCurrency::read(&path,"test",Some("digest"));assert_eq!(c.release_seq_seen,Some(8));assert_eq!(c.running_release_seq,Some(7));
  assert!(ReleaseCurrency::read(&path,"test",Some("different")).running_release_seq.is_none());
  assert!(ReleaseCurrency::read(&path,"wrong",Some("digest")).release_seq_seen.is_none());
  std::fs::remove_dir_all(root).unwrap();
 }
}
