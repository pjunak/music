use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};

const MAX_COUNTER_BYTES: u64 = 4 * 1_024;

pub(crate) fn snapshot(directory: Option<&Path>) -> Value {
    let Some(directory) = directory else {
        return json!({"status": "not_requested"});
    };
    if !cfg!(target_os = "linux") {
        return json!({"status": "unsupported_platform"});
    }
    read_snapshot(directory)
}

fn read_snapshot(directory: &Path) -> Value {
    // Require a v2 marker, including the valid empty controller list at a leaf.
    // The operator selects the scope; never guess the container from a host path.
    if read_counter(directory, "cgroup.controllers").is_none() {
        return json!({"status": "unavailable"});
    }
    let memory_current = read_counter(directory, "memory.current");
    let memory_peak = read_counter(directory, "memory.peak");
    let memory_max = read_counter(directory, "memory.max");
    let cpu_max = read_counter(directory, "cpu.max");
    let cpu_stat = read_counter(directory, "cpu.stat");
    let memory_events = read_counter(directory, "memory.events");
    let current_bytes = memory_current.as_deref().and_then(unsigned);
    let peak_bytes = memory_peak.as_deref().and_then(unsigned);
    let usage_usec = stat(cpu_stat.as_deref(), "usage_usec");
    json!({
        "scope": "Operator-selected cgroup v2. Memory and CPU usage include descendants. Limits are local; peak and counters are cumulative. Null means unavailable.",
        "status": if current_bytes.is_some() || peak_bytes.is_some() || usage_usec.is_some() {
            "observed"
        } else {
            "unavailable"
        },
        "memory": {
            "current_bytes": current_bytes,
            "peak_bytes": peak_bytes,
            "max": memory_limit(memory_max.as_deref()),
        },
        "cpu": {
            "max": cpu_limit(cpu_max.as_deref()),
            "usage_usec": usage_usec,
            "nr_periods": stat(cpu_stat.as_deref(), "nr_periods"),
            "nr_throttled": stat(cpu_stat.as_deref(), "nr_throttled"),
            "throttled_usec": stat(cpu_stat.as_deref(), "throttled_usec"),
        },
        "memory_events": {
            "high": stat(memory_events.as_deref(), "high"),
            "max": stat(memory_events.as_deref(), "max"),
            "oom": stat(memory_events.as_deref(), "oom"),
            "oom_kill": stat(memory_events.as_deref(), "oom_kill"),
        },
    })
}

fn read_counter(directory: &Path, name: &str) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(directory.join(name))
        .ok()?
        .take(MAX_COUNTER_BYTES + 1)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() as u64 <= MAX_COUNTER_BYTES).then_some(value)
}

fn unsigned(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn stat(contents: Option<&str>, key: &str) -> Option<u64> {
    let mut matches = contents?.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == key).then_some(fields)
    });
    let mut fields = matches.next()?;
    let value = unsigned(fields.next()?)?;
    // Ambiguous or malformed counters stay unknown, not first/last-wins.
    (fields.next().is_none() && matches.next().is_none()).then_some(value)
}

fn memory_limit(contents: Option<&str>) -> Value {
    match contents.map(str::trim) {
        Some("max") => json!({"status": "unlimited", "bytes": null}),
        Some(value) if unsigned(value).is_some() => {
            json!({"status": "limited", "bytes": unsigned(value)})
        }
        _ => json!({"status": "unavailable", "bytes": null}),
    }
}

fn cpu_limit(contents: Option<&str>) -> Value {
    let parsed = contents.and_then(|value| {
        let mut fields = value.split_whitespace();
        let quota = fields.next()?;
        let period = unsigned(fields.next()?).filter(|value| *value > 0)?;
        if fields.next().is_some() {
            return None;
        }
        if quota == "max" {
            return Some(json!({"status": "unlimited", "quota_usec": null, "period_usec": period}));
        }
        let quota = unsigned(quota).filter(|value| *value > 0)?;
        Some(json!({"status": "limited", "quota_usec": quota, "period_usec": period}))
    });
    parsed.unwrap_or_else(
        || json!({"status": "unavailable", "quota_usec": null, "period_usec": null}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe_support::ProbeError;

    #[test]
    fn counters_preserve_zero_but_reject_ambiguous_or_malformed_values() {
        assert_eq!(
            stat(Some("usage_usec 123\nnr_throttled 0\n"), "usage_usec"),
            Some(123)
        );
        assert_eq!(
            stat(Some("usage_usec 123\nnr_throttled 0\n"), "nr_throttled"),
            Some(0)
        );
        for contents in [
            None,
            Some(""),
            Some("usage_usec -1"),
            Some("usage_usec +1"),
            Some("usage_usec 1 extra"),
            Some("usage_usec 1\nusage_usec 2"),
            Some("usage_usec 18446744073709551616"),
            Some("usage_usec nan"),
        ] {
            assert_eq!(stat(contents, "usage_usec"), None);
        }
    }

    #[test]
    fn unlimited_and_unknown_limits_are_distinct() {
        assert_eq!(
            memory_limit(Some("4294967296\n")),
            json!({"status": "limited", "bytes": 4294967296_u64})
        );
        assert_eq!(
            memory_limit(Some("max\n")),
            json!({"status": "unlimited", "bytes": null})
        );
        assert_eq!(
            cpu_limit(Some("300000 100000\n")),
            json!({"status": "limited", "quota_usec": 300000, "period_usec": 100000})
        );
        assert_eq!(
            cpu_limit(Some("max 100000\n")),
            json!({"status": "unlimited", "quota_usec": null, "period_usec": 100000})
        );
        for input in [
            None,
            Some(""),
            Some("1 extra"),
            Some("-1"),
            Some("18446744073709551616"),
        ] {
            assert_eq!(memory_limit(input)["status"], "unavailable");
        }
        for input in [
            None,
            Some(""),
            Some("max"),
            Some("1 0"),
            Some("0 100000"),
            Some("max 0"),
            Some("1 100000 extra"),
            Some("NaN 100000"),
            Some("1.5 100000"),
        ] {
            assert_eq!(cpu_limit(input)["status"], "unavailable");
        }
    }

    fn fixture(directory: &Path) -> Result<(), ProbeError> {
        for (name, content) in [
            ("cgroup.controllers", ""),
            ("memory.current", "1048576\n"),
            ("memory.peak", "2097152\n"),
            ("memory.max", "4294967296\n"),
            ("cpu.max", "300000 100000\n"),
            (
                "cpu.stat",
                "usage_usec 1500000\nnr_periods 10\nnr_throttled 2\nthrottled_usec 500\nprivate 99\n",
            ),
            (
                "memory.events",
                "high 1\nmax 0\noom 0\noom_kill 0\nprivate 99\n",
            ),
        ] {
            std::fs::write(directory.join(name), content)?;
        }
        Ok(())
    }

    #[test]
    fn snapshots_read_only_the_selected_v2_scope_without_exposing_paths() -> Result<(), ProbeError>
    {
        let directory = tempfile::tempdir()?;
        fixture(directory.path())?;
        let before = read_snapshot(directory.path());
        assert_eq!(before["status"], "observed");
        assert_eq!(before["memory"]["current_bytes"], 1048576);
        assert_eq!(before["memory"]["peak_bytes"], 2097152);
        assert_eq!(before["memory"]["max"]["bytes"], 4294967296_u64);
        assert_eq!(before["cpu"]["max"]["quota_usec"], 300000);
        assert_eq!(before["cpu"]["usage_usec"], 1500000);
        assert_eq!(before["cpu"]["nr_periods"], 10);
        assert_eq!(before["cpu"]["nr_throttled"], 2);
        assert_eq!(before["cpu"]["throttled_usec"], 500);
        assert_eq!(
            before["memory_events"],
            json!({"high": 1, "max": 0, "oom": 0, "oom_kill": 0})
        );
        assert!(!before.to_string().contains("private"));
        assert!(
            !before
                .to_string()
                .contains(directory.path().to_string_lossy().as_ref())
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("memory.peak"))?,
            "2097152\n"
        );
        std::fs::write(directory.path().join("cpu.stat"), "usage_usec 2100000\n")?;
        let after = read_snapshot(directory.path());
        assert_eq!(after["cpu"]["usage_usec"], 2100000);
        assert!(after["cpu"]["nr_throttled"].is_null());
        assert_eq!(before["cpu"]["usage_usec"], 1500000);
        Ok(())
    }

    #[test]
    fn missing_oversized_and_malformed_counters_remain_unknown() -> Result<(), ProbeError> {
        let directory = tempfile::tempdir()?;
        assert_eq!(read_snapshot(directory.path())["status"], "unavailable");
        std::fs::write(directory.path().join("cgroup.controllers"), "memory cpu\n")?;
        std::fs::write(directory.path().join("memory.current"), "1048576\n")?;
        std::fs::write(directory.path().join("memory.peak"), "2".repeat(4097))?;
        std::fs::write(directory.path().join("cpu.max"), "300000 0\n")?;
        let report = read_snapshot(directory.path());
        assert_eq!(report["memory"]["current_bytes"], 1048576);
        assert!(report["memory"]["peak_bytes"].is_null());
        assert_eq!(report["memory"]["max"]["status"], "unavailable");
        assert_eq!(report["cpu"]["max"]["status"], "unavailable");
        assert!(report["cpu"]["usage_usec"].is_null());
        assert!(report["memory_events"]["oom_kill"].is_null());
        std::fs::write(directory.path().join("cpu.stat"), [b'u', 255])?;
        assert!(read_snapshot(directory.path())["cpu"]["usage_usec"].is_null());
        Ok(())
    }

    #[test]
    fn capture_is_opt_in_and_never_labels_other_platforms_as_linux() {
        assert_eq!(snapshot(None), json!({"status": "not_requested"}));
        if !cfg!(target_os = "linux") {
            assert_eq!(
                snapshot(Some(Path::new("private/cgroup"))),
                json!({"status": "unsupported_platform"})
            );
        }
    }
}
