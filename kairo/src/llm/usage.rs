//! Normalized model-call usage (Phase 13A Day 1).
//!
//! Cached-token fields are `Option` — never silently treat "unsupported" as zero.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Usage for a single model HTTP call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: u64,
    /// `input - cached` when provider includes cached tokens inside `input_tokens`.
    pub fresh_input_tokens: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
    pub total_latency_ms: u64,
    pub estimated_input_cost_usd: Option<f64>,
    pub estimated_cached_input_cost_usd: Option<f64>,
    pub estimated_output_cost_usd: Option<f64>,
    pub estimated_total_cost_usd: Option<f64>,
}

impl Default for ModelUsage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            cached_input_tokens: None,
            output_tokens: 0,
            fresh_input_tokens: None,
            time_to_first_token_ms: None,
            total_latency_ms: 0,
            estimated_input_cost_usd: None,
            estimated_cached_input_cost_usd: None,
            estimated_output_cost_usd: None,
            estimated_total_cost_usd: None,
        }
    }
}

impl ModelUsage {
    /// Derive fresh tokens and apply rough USD estimates from pricing.
    pub fn finalize(&mut self, pricing: &ModelPricing) {
        self.fresh_input_tokens = match self.cached_input_tokens {
            Some(cached) if cached <= self.input_tokens => {
                Some(self.input_tokens.saturating_sub(cached))
            }
            Some(_) => None, // inconsistent provider numbers — leave unavailable
            None => None,
        };
        pricing.apply(self);
    }

    pub fn cache_hit_rate(&self) -> Option<f64> {
        let cached = self.cached_input_tokens?;
        if self.input_tokens == 0 {
            return None;
        }
        Some(cached as f64 / self.input_tokens as f64)
    }
}

/// Aggregated usage for one agent run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageTotals {
    pub model_id: String,
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Sum of cached tokens across calls that reported them.
    pub cached_input_tokens: Option<u64>,
    pub fresh_input_tokens: Option<u64>,
    /// True if every call that reported input also reported cached (or input was 0).
    pub cached_fully_reported: bool,
    pub total_latency_ms: u64,
    pub avg_latency_ms: u64,
    pub avg_time_to_first_token_ms: Option<u64>,
    pub estimated_input_cost_usd: Option<f64>,
    pub estimated_cached_input_cost_usd: Option<f64>,
    pub estimated_output_cost_usd: Option<f64>,
    pub estimated_total_cost_usd: Option<f64>,
}

impl UsageTotals {
    pub fn cache_hit_rate(&self) -> Option<f64> {
        if !self.cached_fully_reported {
            return None;
        }
        let cached = self.cached_input_tokens?;
        if self.input_tokens == 0 {
            return None;
        }
        Some(cached as f64 / self.input_tokens as f64)
    }

    pub fn render_report(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Run Summary".to_string());
        if !self.model_id.is_empty() {
            lines.push(format!("Model: {}", self.model_id));
        }
        lines.push(format!("Calls: {}", self.calls));
        lines.push(String::new());
        lines.push(format!("Input tokens:  {}", self.input_tokens));
        lines.push(format!(
            "Cached input:  {}",
            fmt_opt_u64(self.cached_input_tokens, self.cached_fully_reported)
        ));
        lines.push(format!(
            "Fresh input:   {}",
            fmt_opt_u64(self.fresh_input_tokens, self.cached_fully_reported)
        ));
        lines.push(format!("Output:        {}", self.output_tokens));
        lines.push(String::new());
        lines.push(format!(
            "Cache hit rate: {}",
            match self.cache_hit_rate() {
                Some(r) => format!("{:.0}%", r * 100.0),
                None => "unavailable".to_string(),
            }
        ));
        lines.push(String::new());
        lines.push("Estimated cost:".to_string());
        lines.push(format!(
            "  Input:         {}",
            fmt_opt_usd(self.estimated_input_cost_usd)
        ));
        lines.push(format!(
            "  Cached input:  {}",
            fmt_opt_usd(self.estimated_cached_input_cost_usd)
        ));
        lines.push(format!(
            "  Output:        {}",
            fmt_opt_usd(self.estimated_output_cost_usd)
        ));
        lines.push(format!(
            "  Total:         {}",
            fmt_opt_usd(self.estimated_total_cost_usd)
        ));
        lines.push(String::new());
        lines.push("Average latency:".to_string());
        lines.push(format!(
            "  Time to first token: {}",
            match self.avg_time_to_first_token_ms {
                Some(ms) => format!("{ms} ms"),
                None => "unavailable".to_string(),
            }
        ));
        lines.push(format!(
            "  Total completion:    {} ms (avg/call)",
            self.avg_latency_ms
        ));
        lines.join("\n")
    }
}

fn fmt_opt_u64(v: Option<u64>, fully_reported: bool) -> String {
    match (v, fully_reported) {
        (Some(n), true) => n.to_string(),
        (Some(n), false) => format!("{n}+ (partial)"),
        (None, _) => "unavailable".to_string(),
    }
}

fn fmt_opt_usd(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("${x:.6}"),
        None => "unavailable".to_string(),
    }
}

/// Accumulators for one run.
#[derive(Debug, Clone)]
pub struct UsageAccumulator {
    model_id: String,
    pricing: ModelPricing,
    calls: Vec<ModelUsage>,
}

impl UsageAccumulator {
    pub fn new(model_id: impl Into<String>, pricing: ModelPricing) -> Self {
        Self {
            model_id: model_id.into(),
            pricing,
            calls: Vec::new(),
        }
    }

    pub fn record(&mut self, mut usage: ModelUsage) {
        usage.finalize(&self.pricing);
        self.calls.push(usage);
    }

    pub fn last(&self) -> Option<&ModelUsage> {
        self.calls.last()
    }

    pub fn totals(&self) -> UsageTotals {
        let mut totals = UsageTotals {
            model_id: self.model_id.clone(),
            calls: self.calls.len() as u32,
            cached_fully_reported: !self.calls.is_empty(),
            ..UsageTotals::default()
        };

        let mut ttft_sum = 0u64;
        let mut ttft_n = 0u32;
        let mut any_cost = false;
        let mut input_cost = 0.0;
        let mut cached_cost = 0.0;
        let mut output_cost = 0.0;
        let mut total_cost = 0.0;

        for u in &self.calls {
            totals.input_tokens += u.input_tokens;
            totals.output_tokens += u.output_tokens;
            totals.total_latency_ms += u.total_latency_ms;

            match u.cached_input_tokens {
                Some(c) => {
                    totals.cached_input_tokens =
                        Some(totals.cached_input_tokens.unwrap_or(0) + c);
                }
                None if u.input_tokens > 0 => {
                    totals.cached_fully_reported = false;
                }
                None => {}
            }

            match u.fresh_input_tokens {
                Some(f) => {
                    totals.fresh_input_tokens =
                        Some(totals.fresh_input_tokens.unwrap_or(0) + f);
                }
                None if u.cached_input_tokens.is_some() || u.input_tokens > 0 => {
                    // fresh unavailable for this call
                    if u.cached_input_tokens.is_none() {
                        // already marked partial above
                    }
                }
                None => {}
            }

            if let Some(ms) = u.time_to_first_token_ms {
                ttft_sum += ms;
                ttft_n += 1;
            }

            if let Some(c) = u.estimated_input_cost_usd {
                input_cost += c;
                any_cost = true;
            }
            if let Some(c) = u.estimated_cached_input_cost_usd {
                cached_cost += c;
                any_cost = true;
            }
            if let Some(c) = u.estimated_output_cost_usd {
                output_cost += c;
                any_cost = true;
            }
            if let Some(c) = u.estimated_total_cost_usd {
                total_cost += c;
                any_cost = true;
            }
        }

        if !totals.cached_fully_reported {
            // Still show partial sums when some calls reported cache, but hit rate stays None.
            if totals.fresh_input_tokens.is_some() && totals.cached_input_tokens.is_none() {
                totals.fresh_input_tokens = None;
            }
        }

        if totals.calls > 0 {
            totals.avg_latency_ms = totals.total_latency_ms / u64::from(totals.calls);
        }
        if ttft_n > 0 {
            totals.avg_time_to_first_token_ms = Some(ttft_sum / u64::from(ttft_n));
        }
        if any_cost {
            totals.estimated_input_cost_usd = Some(input_cost);
            totals.estimated_cached_input_cost_usd = Some(cached_cost);
            totals.estimated_output_cost_usd = Some(output_cost);
            totals.estimated_total_cost_usd = Some(total_cost);
        }

        totals
    }
}

/// USD per million tokens. Loaded from env; unset → no cost estimates.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelPricing {
    pub input_per_mtok: Option<f64>,
    pub cached_input_per_mtok: Option<f64>,
    pub output_per_mtok: Option<f64>,
}

impl ModelPricing {
    /// `PRICE_INPUT_PER_MTOK`, `PRICE_CACHED_INPUT_PER_MTOK`, `PRICE_OUTPUT_PER_MTOK`
    pub fn from_env() -> Self {
        Self {
            input_per_mtok: env_f64("PRICE_INPUT_PER_MTOK"),
            cached_input_per_mtok: env_f64("PRICE_CACHED_INPUT_PER_MTOK"),
            output_per_mtok: env_f64("PRICE_OUTPUT_PER_MTOK"),
        }
    }

    fn apply(&self, usage: &mut ModelUsage) {
        let mut any = false;
        let mut total = 0.0;

        if let Some(rate) = self.input_per_mtok {
            // Bill non-cached (fresh) input at full rate when we know fresh; else whole input.
            let billable = usage
                .fresh_input_tokens
                .unwrap_or(usage.input_tokens);
            let c = (billable as f64) * rate / 1_000_000.0;
            usage.estimated_input_cost_usd = Some(c);
            total += c;
            any = true;
        }

        if let (Some(rate), Some(cached)) = (self.cached_input_per_mtok, usage.cached_input_tokens)
        {
            let c = (cached as f64) * rate / 1_000_000.0;
            usage.estimated_cached_input_cost_usd = Some(c);
            total += c;
            any = true;
        }

        if let Some(rate) = self.output_per_mtok {
            let c = (usage.output_tokens as f64) * rate / 1_000_000.0;
            usage.estimated_output_cost_usd = Some(c);
            total += c;
            any = true;
        }

        if any {
            usage.estimated_total_cost_usd = Some(total);
        }
    }
}

fn env_f64(key: &str) -> Option<f64> {
    std::env::var(key).ok()?.parse().ok()
}

/// Parse OpenAI-compatible `usage` object. Missing/unknown fields stay `None` / 0.
///
/// Assumes (when cached is present) that cached tokens are included in `prompt_tokens`
/// / `input_tokens` — standard for OpenAI, Groq, and many gateways.
pub fn parse_openai_usage(usage: &Value) -> ModelUsage {
    let mut out = ModelUsage::default();
    if usage.is_null() || !usage.is_object() {
        return out;
    }

    out.input_tokens = first_u64(usage, &["prompt_tokens", "input_tokens"]).unwrap_or(0);
    out.output_tokens =
        first_u64(usage, &["completion_tokens", "output_tokens"]).unwrap_or(0);

    out.cached_input_tokens = first_u64(
        usage,
        &[
            "cached_tokens",
            "cache_read_input_tokens",
            "prompt_cache_hit_tokens",
        ],
    )
    .or_else(|| {
        usage
            .get("prompt_tokens_details")
            .and_then(|d| first_u64(d, &["cached_tokens", "cache_read_tokens"]))
    })
    .or_else(|| {
        usage
            .get("input_tokens_details")
            .and_then(|d| first_u64(d, &["cached_tokens", "cache_read_tokens"]))
    });

    out
}

fn first_u64(v: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(n) = v.get(*key).and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_i64().map(|i| i.max(0) as u64))
                .or_else(|| x.as_f64().map(|f| f.max(0.0) as u64))
        }) {
            return Some(n);
        }
    }
    None
}

/// Compact one-line log for a single call.
pub fn render_call_line(usage: &ModelUsage) -> String {
    let cached = match usage.cached_input_tokens {
        Some(n) => n.to_string(),
        None => "unavailable".to_string(),
    };
    let fresh = match usage.fresh_input_tokens {
        Some(n) => n.to_string(),
        None => "unavailable".to_string(),
    };
    format!(
        "in={} cached={} fresh={} out={} latency={}ms{}",
        usage.input_tokens,
        cached,
        fresh,
        usage.output_tokens,
        usage.total_latency_ms,
        match usage.estimated_total_cost_usd {
            Some(c) => format!(" cost=${c:.6}"),
            None => String::new(),
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_basic_openai_usage() {
        let u = parse_openai_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120
        }));
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
        assert!(u.cached_input_tokens.is_none());
    }

    #[test]
    fn parse_cached_details() {
        let mut u = parse_openai_usage(&json!({
            "prompt_tokens": 1000,
            "completion_tokens": 50,
            "prompt_tokens_details": { "cached_tokens": 700 }
        }));
        u.finalize(&ModelPricing::default());
        assert_eq!(u.cached_input_tokens, Some(700));
        assert_eq!(u.fresh_input_tokens, Some(300));
    }

    #[test]
    fn unavailable_cached_not_zero() {
        let mut acc = UsageAccumulator::new("m", ModelPricing::default());
        let mut u = parse_openai_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 10
        }));
        u.total_latency_ms = 40;
        acc.record(u);
        let t = acc.totals();
        assert!(t.cached_input_tokens.is_none());
        assert!(!t.cached_fully_reported);
        assert!(t.cache_hit_rate().is_none());
        assert!(t.render_report().contains("Cached input:  unavailable"));
    }

    #[test]
    fn pricing_applies_when_configured() {
        let pricing = ModelPricing {
            input_per_mtok: Some(1.0),
            cached_input_per_mtok: Some(0.1),
            output_per_mtok: Some(2.0),
        };
        let mut u = parse_openai_usage(&json!({
            "prompt_tokens": 1_000_000,
            "completion_tokens": 1_000_000,
            "prompt_tokens_details": { "cached_tokens": 500_000 }
        }));
        u.finalize(&pricing);
        assert_eq!(u.fresh_input_tokens, Some(500_000));
        assert!((u.estimated_input_cost_usd.unwrap() - 0.5).abs() < 1e-9);
        assert!((u.estimated_cached_input_cost_usd.unwrap() - 0.05).abs() < 1e-9);
        assert!((u.estimated_output_cost_usd.unwrap() - 2.0).abs() < 1e-9);
    }
}
