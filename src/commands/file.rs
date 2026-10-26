use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::client::uploads_origin;
use crate::fail::{R, failed, usage};
use crate::json::quote_go;
use crate::run::{Run, dry_run};
use crate::state::{cause, create_file, inside};
use crate::util::*;
use serde_json::json;

pub fn command() -> Cmd {
    let download = Cmd::new("download URL", "Download a file from uploads.linear.app into the current directory")
        .long(
            "Download a file from uploads.linear.app (an image or attachment URL in an issue\n\
or comment) with the API key, into the current directory.\n\
--output defaults to the URL's last path segment. An existing file with the same\n\
content is changed: false; with different content the command fails.\n\
--dry-run downloads to memory and reports what --yes would write.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example("  linearctl file download https://uploads.linear.app/8f…/2c…/a1… --output shot.png --yes")
        .args(Args::Exact(1))
        .run(download)
        .flag(Flag::str("output", "write to `PATH` inside the current directory"))
        .mutating();
    Cmd::group("file", "Download files attached to issues and comments", vec![download])
}

/// URL, if it is a file on Linear's file host: the only place the API key
/// is sent outside the API. Its decoded path.
fn upload_path(s: &str) -> R<String> {
    plain("URL", s)?;
    let origin = uploads_origin();
    let o = parse_url(&origin);
    match (parse_url(s), o) {
        (Some(u), Some(o)) if u.scheme == o.scheme && u.host == o.host && !u.has_user && !u.path.trim_matches('/').is_empty() => {
            Ok(u.path)
        }
        _ => Err(usage(format!("URL must be a file on {origin}; got {}", quote_go(s)))),
    }
}

/// path.Base: the last element of a /-path.
fn base(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { ".".into() } else { "/".into() };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

fn download(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let url = &ctx.args[0];
    let path = upload_path(url)?;
    let mut name = ctx.str("output");
    if name.is_empty() {
        name = base(&path);
    }
    plain("--output", &name)?;
    let cwd = std::env::current_dir().map_err(|e| failed(cause(&e)))?;
    let target = inside(&cwd.to_string_lossy(), &name)?;
    let cl = run.client()?;
    let (data, kind) = cl.download(run, url)?;
    let changed = match std::fs::read(&target) {
        Ok(old) if old == data => false,
        Ok(_) => return Err(failed(format!("{target} exists with other content; pass another --output"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(failed(format!("cannot read {target}: {}", cause(&e)))),
    };
    if changed && !dry {
        create_file(&target, &data)?;
    }
    let verb = match (changed, dry) {
        (true, true) => "would write",
        (true, false) => "downloaded",
        (false, _) => "already there",
    };
    let n = data.len();
    run.output(
        json!({"changed": changed, "dryRun": dry, "path": target, "bytes": n, "contentType": kind}),
        &format!("{verb} {target} ({n} bytes)"),
    )
}
