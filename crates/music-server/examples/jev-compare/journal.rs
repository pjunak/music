//! Shared validation for bounded, non-replayable experiment journals.
use super::*;
use music_application::assistant::{TypedAnswer, typed_answers};
use std::collections::BTreeMap;

pub struct Journal {
    pub answers: BTreeMap<usize, BTreeMap<String, TypedAnswer>>,
    pub usage_by_request: BTreeMap<usize, [Option<u64>; 2]>,
    pub pending: Option<usize>,
    pub complete: bool,
    pub tokens: [u64; 2],
    pub token_reports: [usize; 2],
}

pub fn parse(document: &Value, requests: &[comparison::Comparison], text: &str) -> Result<Journal> {
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().ok_or("missing journal plan")?)?;
    if header["event"] != "plan" || header["plan"] != *document {
        return Err("journal does not match the current offline comparison plan".into());
    }
    let mut journal = Journal {
        answers: BTreeMap::new(),
        usage_by_request: BTreeMap::new(),
        pending: None,
        complete: false,
        tokens: [0; 2],
        token_reports: [0; 2],
    };
    let mut next = 0;
    let mut terminal = false;
    for line in lines {
        if terminal {
            return Err("journal contains events after its terminal checkpoint".into());
        }
        let event: Value = serde_json::from_str(line)?;
        let index = event["index"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok());
        match event["event"].as_str() {
            Some("attempt_started") => {
                let item = requests.get(next).ok_or("too many attempts")?;
                if journal.pending.is_some()
                    || index != Some(next)
                    || event["case_id"] != item.case_id
                    || event["variant"] != item.variant
                {
                    return Err("invalid attempt checkpoint order or identity".into());
                }
                journal.pending = Some(next);
            }
            Some("response") => {
                if journal.pending.is_none() || index != journal.pending {
                    return Err("response without its unique preceding attempt".into());
                }
                let result = &event["result"];
                if result["model"] != MODEL {
                    return Err("response model differs from pinned comparison model".into());
                }
                journal.answers.insert(
                    next,
                    typed_answers(&requests[next].request, result["answers"].clone())?,
                );
                let mut usage = [None; 2];
                for (i, key) in ["input_tokens", "output_tokens"].iter().enumerate() {
                    if let Some(value) = result[*key].as_u64() {
                        journal.tokens[i] = journal.tokens[i]
                            .checked_add(value)
                            .ok_or("token total overflow")?;
                        journal.token_reports[i] += 1;
                        usage[i] = Some(value);
                    } else if !result[*key].is_null() {
                        return Err("invalid token count".into());
                    }
                }
                journal.usage_by_request.insert(next, usage);
                journal.pending = None;
                next += 1;
            }
            Some("stopped") => {
                if journal.pending.is_none() || index != journal.pending {
                    return Err("stop without its preceding attempt".into());
                }
                terminal = true;
            }
            Some("complete") => {
                if journal.pending.is_some()
                    || next != requests.len()
                    || event["requests"].as_u64() != Some(next as u64)
                {
                    return Err("completion checkpoint does not cover the plan".into());
                }
                journal.complete = true;
                terminal = true;
            }
            _ => return Err("unknown journal event".into()),
        }
    }
    Ok(journal)
}
