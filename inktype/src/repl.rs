//! Asynchronous adapter to rat's real CLI resolver.
//!
//! Using `rat run <selector> <source>` rather than a fixed MCP endpoint keeps
//! project-scoped and named runtimes identical to the rat command line.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Clone)]
pub struct Runner {
    rat: PathBuf,
    cwd: PathBuf,
}

#[derive(Debug)]
pub struct Reply {
    pub id: u64,
    pub selector: String,
    pub source_start: usize,
    pub source_end: usize,
    pub source: String,
    pub result: Result<String, String>,
}

impl Runner {
    pub fn from_env() -> Self {
        let rat = std::env::var("INKTYPE_RAT_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/home/root/.local/bin/rat"));
        let cwd = std::env::var("INKTYPE_RAT_CWD")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/home/root/inktype-repl"));
        eprintln!("inktype: rat cli={} cwd={}", rat.display(), cwd.display());
        Self { rat, cwd }
    }

    /// Names shown to the vision model. rat still performs authoritative
    /// resolution, so this list is only a handwriting-recognition hint.
    pub fn runtime_names(&self) -> String {
        let mut names = BTreeSet::from(["py".to_string(), "r".to_string(), "pi".to_string()]);
        if let Ok(output) = Command::new(&self.rat)
            .args(["status", "-v"])
            .current_dir(&self.cwd)
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if line.chars().next().is_some_and(char::is_whitespace) || line.trim().is_empty() {
                    continue;
                }
                let mut fields = line.split_whitespace();
                if let (Some(name), Some(state)) = (fields.next(), fields.next()) {
                    if matches!(state, "running" | "stopped") && valid_selector(name) {
                        names.insert(name.to_string());
                    }
                }
            }
        }
        names.into_iter().collect::<Vec<_>>().join(", ")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        id: u64,
        selector: String,
        source_start: usize,
        source_end: usize,
        execution: String,
        source: String,
        tx: Sender<Reply>,
    ) {
        let this = self.clone();
        std::thread::spawn(move || {
            let result = this
                .run_sync(&selector, &execution)
                .and_then(|output| persist_artifacts(id, &output));
            eprintln!("inktype: rat run={id} selector={selector:?} result={result:?}");
            let _ = tx.send(Reply {
                id,
                selector,
                source_start,
                source_end,
                source,
                result,
            });
        });
    }

    fn run_sync(&self, selector: &str, source: &str) -> Result<String, String> {
        if !valid_selector(selector) {
            return Err(format!("invalid rat runtime name {selector:?}"));
        }
        for attempt in 0..2 {
            let output = Command::new(&self.rat)
                .args(["run", selector, source])
                .current_dir(&self.cwd)
                .env("HOME", "/home/root")
                .output()
                .map_err(|e| format!("start rat: {e}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if output.status.success() {
                return Ok(clean_rat_output(&stdout));
            }
            let error = match (stdout.is_empty(), stderr.is_empty()) {
                (false, false) => format!("{stdout}\n{stderr}"),
                (false, true) => stdout,
                (true, false) => stderr,
                (true, true) => format!("rat exited with {}", output.status),
            };
            // tmux runtimes such as pi can need slightly longer than rat's
            // initial health deadline on this tablet. The server continues
            // starting, so retry the actual run once rather than making the
            // user circle the cell again.
            if attempt == 0 && error.contains("kernel started") && error.contains("not responding")
            {
                std::thread::sleep(Duration::from_secs(4));
                continue;
            }
            return Err(error);
        }
        unreachable!()
    }
}

fn clean_rat_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            !(line.starts_with('✓') && line.contains(" vars"))
                && !line.starts_with("kernel started")
                && !line.starts_with("kernel already running")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn persist_artifacts(id: u64, output: &str) -> Result<String, String> {
    let dir = PathBuf::from("/home/root/inktype-assets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create asset directory: {e}"))?;
    let mut artifact = 0usize;
    let mut lines = Vec::new();
    for line in output.lines() {
        if let Some(path) = line.trim().strip_prefix("__RAT_PLOT__:") {
            let destination = dir.join(format!("plot-{id}-{artifact}.png"));
            std::fs::copy(path, &destination).map_err(|e| format!("copy plot {path:?}: {e}"))?;
            lines.push(format!("[[INKTYPE_IMAGE:{}]]", destination.display()));
            artifact += 1;
        } else if let Some(payload) = line.trim().strip_prefix("__INKTYPE_MATH__:") {
            serde_json::from_str::<serde_json::Value>(payload)
                .map_err(|e| format!("invalid math result JSON: {e}"))?;
            lines.push(format!("[[INKTYPE_MATH:{payload}]]"));
        } else {
            lines.push(line.to_string());
        }
    }
    Ok(lines.join("\n"))
}

pub fn dataframe_code(runtime: &str, name: &str, payload: &str) -> Result<String, String> {
    if !valid_variable(name) {
        return Err(format!("invalid data-frame name {name:?}"));
    }
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("table JSON: {e}"))?;
    let columns = value
        .get("columns")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "table JSON needs columns[]".to_string())?;
    let rows = value
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "table JSON needs rows[]".to_string())?;
    let columns: Vec<String> = columns
        .iter()
        .map(|column| {
            column
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "column names must be strings".to_string())
        })
        .collect::<Result<_, _>>()?;
    let mut records = Vec::new();
    for row in rows {
        let row = row
            .as_array()
            .ok_or_else(|| "every table row must be an array".to_string())?;
        if row.len() != columns.len() {
            return Err("table row width does not match columns".into());
        }
        let record = columns
            .iter()
            .cloned()
            .zip(row.iter().cloned())
            .collect::<serde_json::Map<_, _>>();
        records.push(serde_json::Value::Object(record));
    }
    let records = serde_json::to_string(&records).map_err(|e| e.to_string())?;
    let literal = serde_json::to_string(&records).map_err(|e| e.to_string())?;
    match runtime {
        "py" => Ok(format!(
            "import json, pandas as pd\n{name} = pd.DataFrame(json.loads({literal}))\n{name}"
        )),
        "r" => Ok(format!(
            "{name} <- jsonlite::fromJSON({literal}, simplifyDataFrame=TRUE)\n{name}"
        )),
        _ => Err("data frames currently support py or r".into()),
    }
}

pub fn valid_variable(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value.len() <= 64
}

pub fn valid_selector(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_runtime_names_without_shell_syntax() {
        assert!(valid_selector("py"));
        assert!(valid_selector("my-py-here"));
        assert!(valid_selector("py@project"));
        assert!(!valid_selector("py;reboot"));
        assert!(!valid_selector("../py"));
        assert!(valid_variable("sales_2026"));
        assert!(!valid_variable("2sales"));
        let code = dataframe_code(
            "py",
            "sales",
            r#"{"columns":["month","value"],"rows":[["Jan",12]]}"#,
        )
        .unwrap();
        assert!(code.contains("sales = pd.DataFrame"));
    }
}
