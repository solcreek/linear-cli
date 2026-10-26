use crate::cli::{Args, Cmd, Ctx};
use crate::fail::{R, failed, usage};
use crate::json::{self, quote_go};
use crate::keys::make_dir;
use crate::run::{Run, dry_run};
use crate::state::{cause, home, replace_file, with_lock};
use crate::util::*;
use serde_json::{Value, json};

/// The settings config set takes.
const KEYS: &[&str] = &["default-team"];

pub fn command() -> Cmd {
    let show = Cmd::new("show", "Show the settings and the file they live in")
        .example("  linearctl config show --json")
        .run(show);
    let set = Cmd::new("set KEY VALUE", "Change a setting")
        .long(&format!(
            "Change a setting. Keys: {}.\n\
default-team is the team --team falls back to after LINEARCTL_TEAM.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
            KEYS.join(", ")
        ))
        .example("  linearctl config set default-team ENG --yes")
        .args(Args::Exact(2))
        .run(set)
        .mutating();
    Cmd::group("config", "Show or change settings", vec![show, set])
}

fn show(run: &mut Run, _: &Ctx) -> R {
    let cfg = read_config()?;
    let dir = home("config");
    let path = if dir.is_empty() { String::new() } else { format!("{dir}/config.json") };
    let lines: Vec<String> = KEYS
        .iter()
        .map(|k| format!("{k} = {}", or_dash(printable(cfg.get(*k).and_then(Value::as_str).unwrap_or("")))))
        .collect();
    run.output(json!({"path": path, "settings": cfg}), &lines.join("\n"))
}

fn set(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let (key, mut value) = (ctx.args[0].clone(), ctx.args[1].clone());
    if !KEYS.contains(&key.as_str()) {
        return Err(usage(format!("no setting {}; keys: {}", quote_go(&key), KEYS.join(", "))));
    }
    if !is_team(&value) {
        return Err(usage(format!("default-team must be a team key like ENG; got {}", quote_go(&value))));
    }
    value = value.to_uppercase();
    let dir = home("config");
    if dir.is_empty() {
        return Err(failed("no home directory; set LINEARCTL_HOME or XDG_CONFIG_HOME"));
    }
    let edit = || -> R<bool> {
        let mut cfg = read_config()?;
        let changed = cfg.get(&key).and_then(Value::as_str) != Some(value.as_str());
        if !changed || dry {
            return Ok(changed);
        }
        cfg.insert(key.clone(), json!(value));
        let mut data = json::encode(&Value::Object(cfg), true, &[]);
        data.truncate(data.len() - 1);
        data.push('\n');
        replace_file(&dir, "config.json", data.as_bytes())?;
        Ok(changed)
    };
    let changed = if dry {
        edit()?
    } else {
        make_dir(&dir).map_err(|e| failed(format!("cannot create {dir}: {}", cause(&e))))?;
        with_lock(&dir, "config.json", edit)?
    };
    let verb = match (changed, dry) {
        (true, true) => "would set",
        (true, false) => "set",
        (false, _) => "already",
    };
    run.output(
        json!({"changed": changed, "dryRun": dry, "key": key, "value": value}),
        &format!("{verb} {key} = {value}"),
    )
}
