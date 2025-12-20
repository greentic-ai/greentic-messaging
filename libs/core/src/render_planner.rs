//! Deterministic render planner prototype (capability-driven tiers).
use serde::{Deserialize, Serialize};

use crate::{
    provider_capabilities::ProviderCapabilitiesV1,
    render_plan::{RenderPlan, RenderTier, RenderWarning},
};

/// Minimal policy placeholder for future use.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PlannerPolicy;

/// Convenience constructor for a default policy.
pub fn planner_policy() -> PlannerPolicy {
    PlannerPolicy
}

/// Simplified card input for planning tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCard {
    pub title: Option<String>,
    pub text: Option<String>,
    #[serde(default)]
    pub actions: Vec<PlannerAction>,
    #[serde(default)]
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerAction {
    pub title: String,
    pub url: Option<String>,
}

/// Pure deterministic planner that picks a tier based on capabilities.
pub fn plan_render(
    card: &PlannerCard,
    caps: &ProviderCapabilitiesV1,
    _policy: &PlannerPolicy,
) -> RenderPlan {
    let mut warnings = Vec::<RenderWarning>::new();
    let mut summary_sanitized = false;
    let mut actions_sanitized = false;
    let mut lines = Vec::new();
    if let Some(title) = card.title.as_deref() {
        let (clean, stripped) = sanitize_text(title, caps);
        summary_sanitized |= stripped;
        lines.push(clean);
    }
    if let Some(text) = card.text.as_deref()
        && !text.is_empty()
    {
        let (clean, stripped) = sanitize_text(text, caps);
        summary_sanitized |= stripped;
        lines.push(clean);
    }

    let mut action_titles = Vec::new();
    let mut action_links = Vec::new();
    for action in &card.actions {
        let (title, stripped) = sanitize_text(&action.title, caps);
        actions_sanitized |= stripped;
        action_titles.push(title.clone());
        if let Some(url) = &action.url {
            action_links.push(format!("{} ({})", title, url));
        } else {
            action_links.push(title);
        }
    }

    if !action_links.is_empty() {
        lines.push(format!("Actions: {}", action_links.join(", ")));
    }

    let mut summary = if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    };

    if summary_sanitized {
        push_warning(
            &mut warnings,
            "formatting_stripped",
            Some("markdown/html stripped".into()),
            Some("/summary_text".into()),
        );
    }

    if actions_sanitized {
        push_warning(
            &mut warnings,
            "formatting_stripped",
            Some("markdown/html stripped".into()),
            Some("/actions".into()),
        );
    }

    // Truncate text fields based on capabilities.
    if let Some(max) = effective_max_text_len(caps)
        && let Some(text) = &summary
    {
        let (truncated, did_truncate) = truncate_chars(text, max);
        if did_truncate {
            push_warning(
                &mut warnings,
                "text_truncated",
                Some(format!("summary trimmed to {} chars", max)),
                Some("/summary_text".to_string()),
            );
        }
        summary = Some(truncated);
    }

    if let Some(max_bytes) = effective_max_payload_bytes(caps)
        && let Some(text) = &summary
    {
        let (trimmed, did_trim) = truncate_bytes(text, max_bytes);
        if did_trim {
            push_warning(
                &mut warnings,
                "payload_trimmed",
                Some(format!("summary trimmed to {} bytes", max_bytes)),
                Some("/summary_text".to_string()),
            );
        }
        summary = Some(trimmed);
    }

    let tier = select_tier(card, caps, &mut warnings);

    RenderPlan {
        tier,
        summary_text: summary,
        actions: action_titles,
        attachments: card.images.clone(),
        warnings,
        debug: Some(serde_json::json!({
            "planner_version": 1,
            "tier": tier_label(tier),
        })),
    }
}

fn tier_label(tier: RenderTier) -> &'static str {
    match tier {
        RenderTier::TierA => "a",
        RenderTier::TierB => "b",
        RenderTier::TierC => "c",
        RenderTier::TierD => "d",
    }
}

fn select_tier(
    card: &PlannerCard,
    caps: &ProviderCapabilitiesV1,
    warnings: &mut Vec<RenderWarning>,
) -> RenderTier {
    // Tier A path: adaptive cards supported and no unsupported elements.
    if caps.supports_adaptive_cards {
        let unsupported = has_unsupported_elements(card, caps, warnings);
        if !unsupported {
            return RenderTier::TierA;
        }
        // Adaptive is supported but we need to drop/alter elements.
        return RenderTier::TierB;
    }

    // Default to Tier D for everything else.
    let _ = has_unsupported_elements(card, caps, warnings);
    warnings.push(RenderWarning {
        code: "adaptive_cards_not_supported".into(),
        message: None,
        path: None,
    });
    RenderTier::TierD
}

fn has_unsupported_elements(
    card: &PlannerCard,
    caps: &ProviderCapabilitiesV1,
    warnings: &mut Vec<RenderWarning>,
) -> bool {
    let mut unsupported = false;

    if !caps.supports_buttons && !card.actions.is_empty() {
        unsupported = true;
        warnings.push(RenderWarning {
            code: "unsupported_element".into(),
            message: Some("buttons/actions not supported".into()),
            path: Some("/actions".into()),
        });
    }

    if !caps.supports_images && !card.images.is_empty() {
        unsupported = true;
        warnings.push(RenderWarning {
            code: "images_not_supported".into(),
            message: Some("images not supported".into()),
            path: Some("/images".into()),
        });
    }

    unsupported
}

fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx == max {
            return (out, true);
        }
        out.push(ch);
    }
    (out, false)
}

fn truncate_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut out = String::new();
    let mut bytes = 0;
    for ch in text.chars() {
        let len = ch.len_utf8();
        if bytes + len > max {
            break;
        }
        out.push(ch);
        bytes += len;
    }
    (out, true)
}

fn sanitize_text(text: &str, caps: &ProviderCapabilitiesV1) -> (String, bool) {
    let mut sanitized = text.to_string();
    let mut stripped = false;

    if !caps.supports_html {
        let mut out = String::with_capacity(sanitized.len());
        let mut in_tag = false;
        for ch in sanitized.chars() {
            match ch {
                '<' => {
                    in_tag = true;
                }
                '>' => {
                    in_tag = false;
                    continue;
                }
                _ => {
                    if !in_tag {
                        out.push(ch);
                    }
                }
            }
        }
        if out != sanitized {
            stripped = true;
            sanitized = out;
        }
    }

    if !caps.supports_markdown {
        let replaced = sanitized.replace(['*', '_', '`'], "");
        if replaced != sanitized {
            stripped = true;
            sanitized = replaced;
        }
    }

    (sanitized, stripped)
}

fn push_warning(
    warnings: &mut Vec<RenderWarning>,
    code: &str,
    message: Option<String>,
    path: Option<String>,
) {
    if warnings.iter().any(|w| w.code == code && w.path == path) {
        return;
    }
    warnings.push(RenderWarning {
        code: code.to_string(),
        message,
        path,
    });
}

fn effective_max_text_len(caps: &ProviderCapabilitiesV1) -> Option<usize> {
    caps.limits
        .max_text_len
        .or(caps.max_text_len)
        .map(|v| v as usize)
}

fn effective_max_payload_bytes(caps: &ProviderCapabilitiesV1) -> Option<usize> {
    caps.limits
        .max_payload_bytes
        .or(caps.max_payload_bytes)
        .map(|v| v as usize)
}
