use std::collections::BTreeMap;

use gsm_core::{MessageEnvelope, OutMessage};
use sha2::{Digest, Sha256};

pub fn state_hash_parts(tenant: &str, platform: &str, chat_id: &str, msg_id: &str) -> String {
    let mut canonical = BTreeMap::new();
    canonical.insert("chat_id", chat_id);
    canonical.insert("msg_id", msg_id);
    canonical.insert("platform", platform);
    canonical.insert("tenant", tenant);
    let payload = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

pub fn state_hash_out(out: &OutMessage) -> String {
    let msg_id = out.message_id();
    state_hash_parts(&out.tenant, out.platform.as_str(), &out.chat_id, &msg_id)
}

pub fn state_hash_envelope(env: &MessageEnvelope) -> String {
    state_hash_parts(
        &env.tenant,
        env.platform.as_str(),
        &env.chat_id,
        &env.msg_id,
    )
}
